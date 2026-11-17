# Separation benchmark (Stage 0, experiment 1)

Status: **measured on this machine, 17 November 2026.** Every number below was produced
by `studio/bench/separation_bench.py` on the RTX 5070 Ti; nothing is copied from a
leaderboard except where a row says "MVSEP". Raw per-track results (JSON) are in
`$JK_BUILD/eval/results/` (outside the repo). Model licences and hashes: `MODELS.md`.
The quality screening ran 07:00-12:00; a second session (15:19-16:46) re-ran the
engineering checks from scratch (section 6.8 lists what reproduced), added a float
source with stems above full scale to the "nothing lost" check, and measured the two
best karaoke models at their shipped overlaps over the whole ccMixter set.

## Summary

- **It runs, on the GPU, in full precision.** PyTorch 2.14.1+cu130 on the RTX 5070 Ti,
  fp32 with TF32 off. The network code reproduces the upstream MSST code bit for bit on
  all 16 RoFormer/SCNet checkpoints, and repeated runs give identical stems.
- **Default preset `full`** (every model allows commercial use, 0.94 GB download):
  vocals = anvuew BS-RoFormer ft1 (GPL-3.0); lead vs backing = anvuew BS-RoFormer
  Karaoke (GPL-3.0) on those vocals; drums = SYH99999 BS-RoFormer 4-stem (Apache-2.0) on
  `mix - vocals`; `other = original - lead - backing - drums`.
- **Quality** (MUSDB18-HQ test, 60 s excerpts; ccMixter lead/backing set, 45 s):
  vocals SDR 13.92 dB, instrumental by subtraction 19.29 dB with the vocal left in it at
  -20.8 dB. The best downloadable model (Leap Xe, no licence) is 0.23 dB better. Lead vs
  backing is the weakest step: lead SDR 10.78 dB in mixes at the shipped overlap 8,
  0.55 dB ahead of the best unlicensed karaoke model, which in turn leaves 1 dB less
  lead in the karaoke mix. Default karaoke mix (backing vocals kept): SDR 18.10 dB, lead
  residue -15.9 dB. Drums 11.22 dB.
- **"full" beats "clean"** (a direct-instrumental model) on SDR (18.10 vs 16.24 dB) and
  on ghost vocal (-15.9 vs -14.8 dB; backing off -24.3 vs -21.7 dB) on both data sets;
  "clean" keeps 0.3-0.6 dB more energy at 4-16 kHz. Default: `full`. `clean` is offered
  as a preset with unwa's Resurrection Inst, which publishes no licence.
- **Speed:** a 4:04 stereo song takes **152-160 s** of separation (two runs; plus 2 s
  warm / 22 s cold to load the models once), 4.1 GB of VRAM. The karaoke model at
  overlap 8 is 99-104 s of that.
- **Nothing lost:** the stems add up to the original within float32 rounding (5.96e-8);
  stored as 24-bit FLAC in a real `.jksong` with the writer's exact-residual mode they
  decode back to the MUSDB originals bit for bit. For a float source loud enough that 26
  of 32 stems need a stored gain (+9 dB), the decoded stems sum back within half a
  24-bit step of the residual stem (max 1.3e-7, SNR >= 134.5 dB).
- **48 kHz sources:** only the model input and estimate are resampled (soxr VHQ); the
  vocal SDR stays within 0.03 dB of the native 44.1 kHz run and the stems stay at 48 kHz.
- **Precision:** TF32 and fp16 change the output by -70 and -65 dB (fp16 is twice as
  fast). Not identical, so songs use fp32. bf16 breaks these models (-7 dB).
- **For the owner:** the default needs no licence decision. The better-scoring
  unlicensed models (`BEST_MEASURED`) and the `clean` preset do; see `MODELS.md`.

## 1. Set-up

| | |
|---|---|
| GPU | NVIDIA GeForce RTX 5070 Ti, 16 GB (sm_120), driver 621.24 |
| CPU / RAM | Ryzen 7 9700X, 32 GB |
| Software | Python 3.12.16, PyTorch 2.14.1+cu130, demucs 4.1.0, soxr 1.1.0, numpy 2.5.5 |
| Precision | fp32 with TF32 **off** (`torch.backends.cuda.matmul.allow_tf32 = False`, cuDNN likewise, matmul precision "highest"), unless a row says otherwise |
| Chunking | Overlap-add as in ZFTurbo's MSST `demix` (each model's own chunk size, linear fades, reflect padding). Quality screening at overlap 2; the overlap sweep is in section 6.2 |
| Network code | `studio/jukeoke_studio/modelzoo/roformer.py`, `scnet.py`: bit-identical outputs to the upstream MSST code on every checkpoint (section 6.7) |

**GPU sharing.** The alignment workstream ran its own GPU jobs on the same card during
the morning quality runs (sections 5.1 to 5.5 were measured between 07:00 and 09:30).
Quality numbers are unaffected. All speed numbers in this document come from dedicated
runs later in the day, one model at a time, with the card idle at the start of each
run (the `nvidia-smi` reading is printed with each table); tables mark any figure that
does not (`*`).

## 2. State of the art, November 2026 (research)

Sources checked on 2026-11-17: MVSEP quality-checker leaderboards (Multisong, Lead/Back
Vocals), ZFTurbo's `docs/pretrained_models.md`, python-audio-separator 0.48.0's model
list and score file, and the Hugging Face pages of each author. Licences were read from
each model's repository metadata and model card; "none" means no licence is published.

**(a) Vocals vs instrumental.** MVSEP's Multisong board (100 one-minute songs, hidden
stems, SDR as in the Music Demixing Challenge) is led by MVSep's own BS-RoFormer 124
bands (vocals 12.33 dB, instrumental 18.64 dB) and its ensembles. These are only offered
through the paid mvsep.com service: no weights, unusable offline. The best downloadable
models there: unwa's **BS-RoFormer Leap Xe** (11.79 dB vocals, no licence), unwa Leap
(11.72), **anvuew BS-RoFormer ft1** (11.54 at overlap 4, **GPL-3.0**), unwa HyperACE v2
(11.40, no licence, needs modified code). Older references: viperx BS-RoFormer 1297
(no licence), Kim's Mel-Band RoFormer (MIT), HT-Demucs ft (MIT).

**(b) Lead vs backing ("karaoke").** MVSEP's Lead/Back board: "Mel-RoFormer (Karaoke /
Duet)" 11.08 dB lead (origin not stated), MVSep's own Lead/Back BS-RoFormer 124 bands
(10.77, service only), community fine-tunes "tf-frazer" (10.53), **BS-RoFormer Karaoke
by anvuew** (10.23, **GPL-3.0**), karaoke by Gabox (10.21, no licence), and the
frazer/becruily BS-RoFormer and becruily Mel-RoFormer karaoke models (no licence) which
appear in most top ensembles. The only karaoke model trained on openly licensed data that
we found (musetric's duality-kara2 student, Apache-2.0, CC data only) reports 6.2 dB lead
SDR on its own ccMixter test, far below.

**(c) Drums.** Multisong drums: MVSEP ensembles 14.35 dB; "BS-RoFormer SW" 6-stem 14.11
dB, which MVSEP's own board lists with identical scores to a "Logic Pro BS-RoFormer" /
`bs_logic_6stem.ckpt`, i.e. it appears to be extracted from Apple's Logic Pro (no licence:
excluded); SCNet XL IHF (ZFTurbo, trained on MUSDB18 only) 11.58; HT-Demucs ft drums
(MIT) 11.13.

**(d) Direct instrumental ("inst") models.** unwa BS-RoFormer Large-Inst v2 (17.61 dB
instrumental, no licence, modified code), becruily Deux (17.55, CC BY-NC 4.0:
non-commercial), unwa HyperACE v2 inst (17.40), becruily instrumental, Gabox INST/FV
series, unwa Mel inst v1e+ (no licence). Note that the best *vocal* models give a higher
instrumental SDR on that board by subtraction (Leap Xe: 18.10 dB) than any inst model.

**Licences in one sentence.** Almost all top community checkpoints publish no licence
(all rights reserved by default). The permissive or GPL options of near-top quality are
anvuew's two GPL-3.0 models (vocals, karaoke), Kim's MIT Mel-RoFormer and its Apache-2.0
fine-tune Duality, HT-Demucs (MIT) and SYH99999's Apache-2.0 4-stem BS-RoFormer. Full
table: `MODELS.md`.

## 3. Evaluation sets (never committed; under `$JK_BUILD/eval`)

| set | content | source | licence | used for |
|---|---|---|---|---|
| MUSDB18-HQ, test split | 50 songs, true vocals / drums / bass / other, 44.1 kHz WAV | Zenodo record 3338373 (official), fetched by HTTP range requests so only the 7.9 GB test half was downloaded | "educational purposes only", non-commercial; DSD100 / MedleyDB (CC BY-NC-SA) material | vocals, instrumental, drums, precision, presets' "nothing lost", 48 kHz |
| ccMixter lead/backing | 41 a cappellas whose **lead and backing vocals were uploaded as separate files**, 18 of them with the uploader's own instrument stems | ccmixter.org, list of uploads and which file is which from musetric/lab `ccmixter_voices.json` (MIT); 1.5 GB of source files | each upload CC BY 2.5 / 3.0 / 4.0 (29 x CC BY 3.0, 10 x 4.0, 2 x 2.5; 10 uploaders); attribution table in `$JK_BUILD/eval/ccmixter_voices/LICENSE-EVAL.md` | karaoke models, full vs clean karaoke mixes |

Both are fetched by `studio/bench/fetch_eval.py` (total download 9.4 GB). MP3 sources in
the ccMixter set are decoded once to float32; the ground truth is that decoded audio, so
the evaluation is exact even though the source files are lossy. In the 45 s excerpts
used, the median backing level is -4.8 dB relative to all vocals, so backing vocals are
substantial. Set and licences: `$JK_BUILD/eval/*/LICENSE-EVAL.md`.

**Excerpts.** Each track is evaluated on one window starting at 30 % of its length, 60 s
(MUSDB) or 45 s (ccMixter), the same window for every model. MVSEP's Multisong set also
uses one-minute clips. The GPU budget did not allow full songs for every candidate; the
whole-song timing (6.6) uses a complete 4-minute track.

**Caveat on MUSDB.** Community models do not disclose their training data; the MUSDB18
test songs may have been in some of them, which would flatter those models. SCNet XL IHF
(ZFTurbo) is the only one here documented as never having seen the test split.

## 4. Metrics

All in `studio/jukeoke_studio/quality.py`, unit-tested in `studio/tests/test_quality.py`.

- **SDR** (= MVSEP's "SDR", the MDX challenge's utterance-level SDR, "uSDR" when averaged):
  `10 log10(sum(s^2) / sum((s - s_hat)^2))` over the whole excerpt, both channels;
  tables give the mean over tracks.
- **cSDR**: the same ratio on 1 s chunks, median over chunks whose reference is not
  silent, then median over tracks. It mirrors SiSEC'18's cSDR (median of 1 s frames,
  median over tracks) but uses the plain SDR ratio instead of BSS-Eval v4's
  distortion-filter projection, so it is not numerically identical to museval's cSDR.
- **Vocal (lead) residue**, dB relative to the true vocal (lead): the estimation error
  `est - true` is projected, per STFT bin and per 4 s segment, onto the true vocal with a
  least-squares complex gain; residue = energy of that projection / energy of the vocal.
  It measures the "ghost singer" left in an instrumental or karaoke mix. Validated on
  synthetic signals with a known leak: -10 / -20 / -30 dB measured as -10.0 / -19.9 /
  -29.4 dB; with no leak at all but independent error at -15 dB the reading is -38.5 dB,
  which is the metric's floor in that situation.
- **Band energy**: estimated vs true energy in < 250 Hz, 250 Hz-4 kHz, 4-8 kHz and
  8-16 kHz (dB; negative = energy lost, e.g. dull cymbals). Computed from a 4096-point
  STFT summed over the excerpt.
- **GPU s/min**: model time (synchronised) per minute of input audio; excludes I/O and
  resampling. **VRAM**: `torch.cuda.max_memory_reserved` during the run (PyTorch's
  allocator; the desktop's own use of the card comes on top).


## 5. Quality results

### 5.1 Vocals (and the "full" instrumental: original minus vocals)

**Vocal models (instrumental = mix - vocals: the 'full' strategy)**

MUSDB18-HQ test, 60 s excerpts, the 25 tracks every model ran on (columns marked 50: all 50 test tracks, where run). Bands: estimated instrumental energy relative to the true instrumental, dB. Left out (no vocals in the excerpt): Skelpolu - Resurrection.

| model | licence (commercial) | vocals SDR | vocals SDR 50 | vocals cSDR | inst SDR | inst SDR 50 | inst cSDR | vocal residue in inst, dB | <250 Hz | 250-4k | 4-8k | 8-16k | GPU s/min | VRAM MB |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| `bs_roformer_leap_xe_vocals` | none published (unclear) | 14.15 | - | 14.41 | 19.52 | - | 20.84 | -21.0 | -0.01 | -0.17 | -0.22 | -0.31 | 20.4 | 3918 |
| `bs_roformer_anvuew_ft1` | GPL-3.0 (yes) | 13.92 | 13.28 | 14.18 | 19.29 | 19.18 | 20.47 | -20.8 | -0.01 | -0.18 | -0.29 | -0.40 | 10.2 | 2992 |
| `bs_roformer_viperx_1297` | none published (unclear) | 13.63 | - | 13.86 | 19.00 | - | 20.27 | -20.7 | -0.01 | -0.15 | -0.18 | -0.26 | 10.9 | 2590 |
| `mel_roformer_kim` | MIT (yes) | 13.29 | - | 13.57 | 18.66 | - | 19.92 | -19.9 | -0.01 | -0.19 | -0.16 | -0.25 | 5.5 | 2336 |
| `mel_roformer_duality` | Apache-2.0 (yes) | 13.18 | - | 13.32 | 18.55 | - | 19.66 | -20.7 | -0.02 | -0.31 | -0.45 | -0.62 | 6.0 | 3516 |
| `mel_roformer_becruily_vocals` | none published (unclear) | 13.06 | - | 13.30 | 18.43 | - | 19.57 | -20.2 | -0.03 | -0.27 | -0.37 | -0.51 | 4.7 | 2336 |
| `htdemucs_ft_vocals` | MIT (yes) | 9.87 | - | 9.99 | 15.24 | - | 16.46 | -15.5 | -0.03 | -0.37 | -0.29 | -0.34 | 0.8 | 1106 |

- unwa's **Leap Xe** scores best (+0.23 dB vocals and instrumental over anvuew ft1) but
  publishes no licence and costs twice the GPU time. **anvuew BS-RoFormer ft1 (GPL-3.0)**
  is within a quarter of a dB and is the best model whose licence allows commercial use:
  it is the default. viperx's older BS-RoFormer 1297 is 0.29 dB behind it (no licence),
  Kim's MIT Mel-RoFormer 0.63 dB, Apache-2.0 Duality 0.74 dB, HT-Demucs 4 dB.
- GPU s/min and VRAM in this table come from the dedicated speed run (section 6.5).
- Instrumental by subtraction (`mix - vocals`) loses very little of the band: 0.01 dB
  below 250 Hz, 0.18 dB in the mids and 0.4 dB in 8-16 kHz for the default model,
  with the vocal left in it at -20.8 dB relative to the true vocal.
- The 25-track mean is 0.6 dB above the 50-track mean for anvuew ft1 (13.92 vs 13.28):
  the other 25 test tracks are harder. Compare models on the same 25 tracks only.

### 5.2 Direct instrumental models (the "clean" strategy) on MUSDB18-HQ

**Instrumental models (the 'clean' strategy)**

MUSDB18-HQ test, 60 s excerpts, the 25 tracks every model ran on (columns marked 50: all 50 test tracks, where run). Bands: estimated instrumental energy relative to the true instrumental, dB. Left out (no vocals in the excerpt): Skelpolu - Resurrection.

| model | licence (commercial) | vocals SDR | vocals SDR 50 | vocals cSDR | inst SDR | inst SDR 50 | inst cSDR | vocal residue in inst, dB | <250 Hz | 250-4k | 4-8k | 8-16k | GPU s/min | VRAM MB |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| `bs_inst_resurrection_unwa` | none published (unclear) | 12.71 | - | 12.59 | 18.08 | - | 19.06 | -19.3 | -0.05 | -0.14 | -0.14 | -0.18 | 9.6 | 2728 |
| `mel_inst_unwa_v1e_plus` | none published (unclear) | 12.08 | - | 11.83 | 17.45 | - | 18.47 | -17.4 | -0.05 | -0.11 | -0.04 | -0.04 | 5.2 | 2820 |
| `mel_inst_becruily` | none published (unclear) | 11.98 | - | 11.59 | 17.35 | - | 18.39 | -17.5 | -0.05 | -0.11 | -0.07 | -0.13 | 4.8 | 2336 |

Against the default vocal model's `mix - vocals` (instrumental SDR 19.29 dB, vocal
residue -20.8 dB, 8-16 kHz -0.40 dB), every direct-instrumental model is worse on SDR
(-1.2 to -1.9 dB) and leaves **more** of the voice in (-17.4 to -19.3 dB), but keeps
more of the highs: unwa v1e+ loses only 0.04 dB in 4-16 kHz, Resurrection 0.14 / 0.18 dB.
So on this data the WORK_ORDER's expected trade-off is reversed for residue: "full" has
both the higher SDR and the cleaner result; "clean" keeps the cymbals a little brighter.

### 5.3 Lead versus backing vocals (karaoke models), ccMixter set

**Lead / backing (karaoke models), ccMixter set**

41 a cappellas (45 s excerpts), 18 of them also over their own instrument stems. Pipeline: vocal model `bs_roformer_anvuew_ft1`, then the karaoke model on its vocals. Direct: karaoke model on the full mix. Karaoke mix = mix - estimated lead (backing vocals kept).

| model | licence (commercial) | a cappella: lead SDR (mean / median) | backing SDR | lead residue in backing, dB | mix: lead SDR (pipeline) | lead SDR (direct) | backing SDR | karaoke-mix SDR | lead residue in karaoke mix, dB | GPU s/min | VRAM MB |
|---|---|---|---|---|---|---|---|---|---|---|---|
| `bs_karaoke_frazer_becruily` | none published (unclear) | 10.44 / 6.62 | 6.02 | -15.3 | 10.38 | 10.66 | 7.75 | 17.69 | -17.1 | 8.3 | 2766 |
| `bs_karaoke_anvuew` | GPL-3.0 (yes) | 10.24 / 6.61 | 4.78 | -14.8 | 10.38 | 9.76 | 5.93 | 17.69 | -15.3 | 7.6 | 2080 |
| `mel_karaoke_aufr33_viperx` | none published (unclear) | 9.02 / 4.95 | 3.12 | -18.3 | 9.47 | 8.87 | 4.55 | 16.78 | -15.5 | 5.6 | 2336 |
| `bs_karaoke_gabox_is` | none published (unclear) | 9.76 / 5.98 | 5.48 | -14.0 | 9.34 | 10.32 | 6.99 | 16.65 | -14.9 | 6.4 | 1284 |
| `mel_karaoke_becruily` | none published (unclear) | 9.82 / 6.00 | 3.72 | -16.6 | 9.10 | 9.36 | 4.98 | 16.41 | -15.8 | 5.5 | 3636 |

All rows above at overlap 2 (the screening setting). The karaoke models turned out to be
the one place where chunk overlap matters (section 6.2), so the two best were run again
over the whole set at the overlap each one ships with (second session, 15:46-16:42):

**Lead / backing at the shipped overlaps (`bs_karaoke_anvuew` 8, `bs_karaoke_frazer_becruily` 4), ccMixter set, same 41 / 18 tracks**

| model | licence (commercial) | a cappella: lead SDR (mean / median) | backing SDR | lead residue in backing, dB | mix: lead SDR (pipeline) | lead SDR (direct) | backing SDR | karaoke-mix SDR | lead residue in karaoke mix, dB | GPU s/min | VRAM MB |
|---|---|---|---|---|---|---|---|---|---|---|---|
| `bs_karaoke_anvuew` (overlap 8) | GPL-3.0 (yes) | 10.61 / 6.70 | 5.14 | -15.0 | **10.78** | 10.23 | 6.34 | **18.10** | -15.9 | 35.3 | 2140 |
| `bs_karaoke_frazer_becruily` (overlap 4) | none published (unclear) | 10.44 / 7.76 | 6.05 | -15.0 | 10.23 | 10.68 | **7.53** | 17.54 | **-16.9** | 19.6 | 2838 |

GPU s/min and VRAM in this table are from these quality runs (a cappella passes), not
from the dedicated speed run of section 6.5.

Paired per track (same tracks, same inputs):

- **anvuew, overlap 8 vs 2:** +0.36 dB a cappella lead SDR (better on 30 of 41), +0.41 dB
  lead SDR in mixes (16 of 18), +0.41 dB backing SDR (13 of 15), lead residue in the
  karaoke mix -0.55 dB (lower on 11 of 18). Half the +0.72 dB the 10-track sweep
  suggested, still well above the 0.1 dB rule: overlap 8 stays.
- **frazer & becruily (4) minus anvuew (8):** lead SDR in mixes -0.56 dB mean / -0.13 dB
  median (frazer better on 7 of 18; karaoke-mix SDR moves identically, its error is the
  lead error), backing SDR +1.19 dB
  mean / +0.29 dB median (better on 8 of 15), lead residue in the karaoke mix -1.0 dB
  mean / -0.26 dB median (lower on 12 of 18), a cappella lead SDR -0.17 dB mean /
  +0.36 dB median (better on 27 of 41).

- The lead/backing split is the weakest link of the chain: lead SDR about 10.8 dB in
  mixes, against 13.9 dB for vocals vs band. Whatever lead the karaoke model misses ends
  up in the backing stem, which stays in the karaoke mix: that is the main source of
  "ghost singer" in the default mix (section 5.4).
- At the shipped settings **anvuew (GPL-3.0)** is ahead of **frazer & becruily (no
  licence)** on lead SDR in mixes (10.78 vs 10.23 dB) and on karaoke-mix SDR (18.10 vs
  17.54 dB). frazer & becruily keeps the backing stem better (7.53 vs 6.34 dB backing
  SDR) and leaves about 1 dB less lead in the karaoke mix (-16.9 vs -15.9 dB mean), on a
  majority of tracks but with small medians. Neither is better on every measure; the
  licence decides: anvuew is the default, frazer & becruily stays in `BEST_MEASURED` for
  a comparison by ear (lead residue is the measure closest to an audible "ghost singer",
  but only listening can say whether 1 dB there outweighs 0.55 dB of lead SDR).
  At overlap 2 for both (first table) they tied on lead SDR (10.38 dB) and karaoke-mix
  SDR (17.69 dB).
- Running the karaoke model on the vocal model's output (the pipeline) beats running it
  on the full mix for anvuew (10.78 vs 10.23 dB lead SDR at overlap 8; 10.38 vs 9.76 dB at
  overlap 2), so the presets do that. For frazer & becruily it is the other way round
  (10.68 direct vs 10.23 pipeline at overlap 4).
- Backing vocals in this set are substantial (median -4.8 dB relative to all vocals);
  the set was built from uploads whose lead and backing parts are separate files.

### 5.4 "full" versus "clean" karaoke mixes, with the true stems known

**'full' versus 'clean' karaoke mixes (ccMixter mixes, true stems known; overlap 2 for every model)**

18 mixes. Vocal model `bs_roformer_anvuew_ft1`, karaoke model `bs_karaoke_anvuew`. Backing on: full = mix - lead; clean = instrumental model + backing. Backing off: full = mix - vocals; clean = instrumental model. Residue: lead (backing on) or all vocals (backing off) left in the mix, dB relative to that source (mean / median over tracks). Bands: estimated vs true karaoke mix, dB, **median** over tracks (the mean is dominated by one track whose instrument stem the models largely take for voice: down to -51 dB in a band).

| variant | backing | SDR mean | SDR median | residue, dB mean / median | <250 Hz | 250-4k | 4-8k | 8-16k |
|---|---|---|---|---|---|---|---|---|
| full | on | 17.69 | 19.41 | -15.3 / -12.6 | -0.01 | -0.04 | -0.53 | -0.81 |
| clean:mel_inst_becruily | on | 14.87 | 15.96 | -13.2 / -11.0 | -0.01 | -0.03 | -0.24 | -0.61 |
| clean:mel_inst_unwa_v1e_plus | on | 15.25 | 16.59 | -13.4 / -10.9 | -0.01 | -0.04 | -0.17 | -0.10 |
| clean:bs_inst_resurrection_unwa | on | 15.87 | 17.06 | -14.3 / -11.9 | -0.02 | -0.05 | -0.18 | -0.16 |
| full | off | 21.12 | 21.50 | -24.3 / -26.3 | -0.01 | -0.05 | -0.43 | -0.72 |
| clean:mel_inst_becruily | off | 17.36 | 15.91 | -19.4 / -19.2 | -0.04 | -0.07 | -0.17 | -0.38 |
| clean:mel_inst_unwa_v1e_plus | off | 17.74 | 15.95 | -19.5 / -18.3 | -0.03 | -0.07 | -0.12 | -0.03 |
| clean:bs_inst_resurrection_unwa | off | 18.61 | 18.86 | -21.7 / -21.9 | -0.04 | -0.07 | -0.19 | -0.18 |

The same comparison with the shipped settings (karaoke model at overlap 8, the others
at 2; only the backing-on rows change, since backing-off does not use the karaoke
model): "full" rises to 18.10 dB SDR and its lead residue falls to -15.9 dB mean /
-15.8 dB median (from -15.3 / -12.6), "clean" to 16.24 dB and -14.8 dB. The order is
unchanged. The price of the better lead removal is a little more high-frequency loss
in "full": median -0.61 dB at 4-8 kHz and -1.05 dB at 8-16 kHz (clean -0.28 / -0.49).

**'full' versus 'clean' karaoke mixes (ccMixter mixes, true stems known; overlap bs_roformer_anvuew_ft1 2, bs_karaoke_anvuew 8, bs_inst_resurrection_unwa 2)**

18 mixes. Vocal model `bs_roformer_anvuew_ft1`, karaoke model `bs_karaoke_anvuew`. Backing on: full = mix - lead; clean = instrumental model + backing. Backing off: full = mix - vocals; clean = instrumental model. Residue: lead (backing on) or all vocals (backing off) left in the mix, dB relative to that source (mean / median over tracks). Bands: estimated vs true karaoke mix, dB, **median** over tracks (the mean is dominated by one track whose instrument stem the models largely take for voice: down to -51 dB in a band).

| variant | backing | SDR mean | SDR median | residue, dB mean / median | <250 Hz | 250-4k | 4-8k | 8-16k |
|---|---|---|---|---|---|---|---|---|
| full | on | 18.10 | 19.48 | -15.9 / -15.8 | -0.01 | -0.04 | -0.61 | -1.05 |
| clean:bs_inst_resurrection_unwa | on | 16.24 | 17.89 | -14.8 / -14.6 | -0.01 | -0.05 | -0.28 | -0.49 |
| full | off | 21.12 | 21.50 | -24.3 / -26.3 | -0.01 | -0.05 | -0.43 | -0.72 |
| clean:bs_inst_resurrection_unwa | off | 18.61 | 18.86 | -21.7 / -21.9 | -0.04 | -0.07 | -0.19 | -0.18 |

In the first table (overlap 2 for every model, three instrumental models):

- **Backing vocals on (the default karaoke mix):** "full" (`mix - lead`) has the higher
  SDR (17.69 vs 14.87-15.87 dB) and the lower lead residue (-15.3 vs -13.2 to -14.3 dB
  mean). Its cost is in the highs: median -0.53 dB at 4-8 kHz and -0.81 dB at 8-16 kHz
  versus -0.17 / -0.10 dB for the best "clean" variants. Most likely the lead estimate
  carries some hi-hat and cymbal energy, and subtracting it takes that out of the band.
- **Backing vocals off:** "full" (`mix - vocals`) again leads on SDR (21.1 vs 17.4-18.6
  dB) and on residue (-24.3 vs -19.4 to -21.7 dB), and again loses more in 8-16 kHz
  (-0.72 vs -0.03 to -0.38 dB median).
- These mixes are made from MP3 uploads and are much less dense than commercial
  recordings; on MUSDB (5.2) the 8-16 kHz difference between the strategies is smaller
  (-0.40 vs -0.18 dB).

**Decision:** the default is **"full"**: higher SDR (18.10 vs 16.24 dB at the shipped
settings) and less ghost vocal on both data sets, and nothing of the band is removed except what the vocal estimate wrongly
contains. **"clean"** stays as a preset with **`bs_inst_resurrection_unwa`**, the direct
instrumental model with the best SDR and the least residue of the three; it is the one
to pick by ear if the cymbals of a "full" mix sound dull. Its licence is unclear.

### 5.5 Drums

**Drums**

MUSDB18-HQ test, first 25 tracks, 60 s excerpts. Inputs: the mix; mix - vocals estimated by `bs_roformer_anvuew_ft1` (what the presets do); the true instrumental (upper bound). Bands: drum estimate vs true drums (from mix - vocals), dB.

| model | licence (commercial) | drums SDR from mix | from mix - vocals | from true inst | cSDR (mix - vocals) | <250 Hz | 250-4k | 4-8k | 8-16k | GPU s/min | VRAM MB |
|---|---|---|---|---|---|---|---|---|---|---|---|
| `scnet_xl_ihf_zfturbo` | MIT (unclear) | 11.84 | 12.00 | 12.46 | 11.66 | -0.18 | -0.77 | -0.93 | -0.59 | 8.5 | 4678 |
| `bs_roformer_4stems_syh` | Apache-2.0 (yes) | 10.78 | 11.22 | 11.62 | 10.93 | -0.30 | -1.21 | -0.70 | -0.70 | 6.8 | 2490 |
| `htdemucs_ft_drums` | MIT (yes) | 9.88 | 10.15 | 10.45 | 9.85 | -0.10 | -1.95 | -1.22 | -0.63 | 0.8 | 1106 |

- SCNet XL IHF is best (12.00 dB from `mix - vocals`) and the only model here known not
  to have seen MUSDB's test songs, so its lead is not an artefact of training data. Its
  weights, though, were trained on MUSDB18-HQ only, whose licence is non-commercial:
  licence "unclear" for our purpose.
- **BS-RoFormer 4 stems by SYH99999 (Apache-2.0)** is 0.78 dB behind and 1.07 dB ahead
  of HT-Demucs (MIT): it is the default. Everything a drum model misses stays in "other"
  (stems still add up), so a weaker drum model only means some drum sound goes through
  the key change, and some non-drum sound does not.
- Feeding the drum model `mix - vocals` instead of the mix helps every model (+0.16 to
  +0.44 dB), as the presets do.

## 6. Engineering checks

### 6.1 Precision: fp32 versus TF32, fp16 and bf16

**Precision: `bs_roformer_anvuew_ft1` (4 tracks, output `vocals`)**

| precision | output difference vs fp32, dB (mean / worst track) | max abs difference | SDR change vs fp32, dB (mean, largest) | GPU s/min |
|---|---|---|---|---|
| fp32 | reference | 0 | 0 | 9.3 |
| tf32 | -70.2 / -66.4 | 8.15e-04 | +3.5e-04 (largest +7.9e-04) | 8.7 |
| fp16 | -65.4 / -61.9 | 1.20e-03 | +4.2e-05 (largest +6.3e-04) | 5.0 |
| bf16 | -7.4 / -6.5 | 3.76e-01 | -8.3e+00 (largest -1.2e+01) | 5.0 |

TF32 and fp16 change the output by -70 dB and -65 dB relative to the fp32 output (peak
differences up to 1.2e-3, i.e. -58 dBFS) and the SDR by less than a thousandth of a dB;
fp16 halves the GPU time. That is close, but it is not the same output, and WORK_ORDER
section 5 allows a faster setting only if it gives the same output, so **fp32 with TF32
off stays the only mode used for songs**. bf16 autocast is unusable for these networks:
the output moves by -7 dB and the SDR drops by 5 to 12 dB (bf16 keeps 8 mantissa bits; a
likely cause is the rotary position angles over a 20 s chunk, not investigated further
since bf16 is not used). The difference measured
for each setting is reproducible with `separation_bench.py precision`.

### 6.2 Chunk overlap

The networks see the song in chunks of 8 to 22 s; with overlap N every sample is
covered by N chunks whose estimates are cross-faded, and the GPU time grows with N.
Each sweep runs the same tracks at every setting; the paired columns compare each track
with itself at overlap 2. The two 6-track MUSDB sweeps of `bs_roformer_anvuew_ft1` and
the morning runs kept only means; the first of them (09:33) shared the GPU, so its GPU
s/min are upper bounds.

**Overlap: `bs_roformer_anvuew_ft1` (12 tracks of MUSDB18-HQ test, 60 s, run 2026-11-17 11:11)**

| overlap | vocals SDR (mean) | change vs overlap 2, dB (paired mean) | tracks better / worse than overlap 2 | GPU s/min |
|---|---|---|---|---|
| 2 | 13.900 | 0 | - | 8.9 |
| 4 | 13.940 | +0.039 | 11 / 1 | 20.0 |
| 8 | 13.929 | +0.029 | 11 / 1 | 41.2 |

**Overlap: `bs_roformer_anvuew_ft1` (6 tracks of MUSDB18-HQ test, 60 s, run 2026-11-17 09:33)**

| overlap | vocals SDR (mean) | change vs overlap 2, dB (paired mean) | tracks better / worse than overlap 2 | GPU s/min |
|---|---|---|---|---|
| 1 | 14.585 | - | - | 3.6 |
| 2 | 14.622 | 0 | - | 9.4 |
| 4 | 14.567 | - | - | 21.2 |
| 8 | 14.526 | - | - | 44.0 |

**Overlap: `bs_karaoke_anvuew` (10 tracks of ccMixter a cappellas, 45 s, run 2026-11-17 10:46)**

| overlap | lead SDR (mean) | change vs overlap 2, dB (paired mean) | tracks better / worse than overlap 2 | GPU s/min |
|---|---|---|---|---|
| 1 | 9.823 | -1.246 | 4 / 6 | 3.6 |
| 2 | 11.069 | 0 | - | 7.8 |
| 4 | 11.599 | +0.529 | 8 / 2 | 16.5 |
| 8 | 11.785 | +0.715 | 8 / 2 | 34.4 |

**Overlap: `bs_karaoke_frazer_becruily` (10 tracks of ccMixter a cappellas, 45 s, run 2026-11-17 11:31)**

| overlap | lead SDR (mean) | change vs overlap 2, dB (paired mean) | tracks better / worse than overlap 2 | GPU s/min |
|---|---|---|---|---|
| 2 | 13.947 | 0 | - | 9.5 |
| 4 | 14.122 | +0.175 | 7 / 2 | 20.1 |
| 8 | 14.190 | +0.243 | 7 / 2 | 42.5 |

**Overlap: `bs_roformer_4stems_syh` (12 tracks of MUSDB18-HQ test, 60 s, run 2026-11-17 10:57)**

| overlap | drums SDR (mean) | change vs overlap 2, dB (paired mean) | tracks better / worse than overlap 2 | GPU s/min |
|---|---|---|---|---|
| 2 | 9.785 | 0 | - | 7.2 |
| 4 | 9.800 | +0.016 | 9 / 3 | 14.9 |
| 8 | 9.860 | +0.076 | 10 / 2 | 30.6 |

**Overlap: `bs_roformer_4stems_syh` (6 tracks of MUSDB18-HQ test, 60 s, run 2026-11-17 10:30)**

| overlap | drums SDR (mean) | change vs overlap 2, dB (paired mean) | tracks better / worse than overlap 2 | GPU s/min |
|---|---|---|---|---|
| 1 | 9.049 | - | - | 3.4 |
| 2 | 9.429 | 0 | - | 7.6 |
| 4 | 9.440 | - | - | 15.3 |
| 8 | 9.525 | - | - | 33.2 |

**Overlap: `bs_inst_resurrection_unwa` (6 tracks of MUSDB18-HQ test, 60 s, run 2026-11-17 10:38)**

| overlap | instrumental SDR (mean) | change vs overlap 2, dB (paired mean) | tracks better / worse than overlap 2 | GPU s/min |
|---|---|---|---|---|
| 1 | 18.586 | - | - | 4.2 |
| 2 | 18.790 | 0 | - | 10.3 |
| 4 | 18.829 | - | - | 22.7 |
| 8 | 18.845 | - | - | 43.3 |

**Rule and result.** A model gets the next higher overlap only if that raises its SDR by
at least 0.1 dB on average over the same tracks (smaller differences are below what
model choice or track choice moves, and they cost two to four times the GPU time).

- Vocals (`bs_roformer_anvuew_ft1`): +0.04 dB at 4 (11 of 12 tracks better), +0.03 dB at
  8; on the first 6 tracks overlap 2 was even the best. **Overlap 2.**
- Drums (`bs_roformer_4stems_syh`): +0.02 dB at 4, +0.08 dB at 8 (10 of 12 better) for
  4.3x the GPU time. **Overlap 2.** (Drum quality only decides what goes through the key
  change; the stems add up either way.)
- Instrumental (`bs_inst_resurrection_unwa`, clean preset): +0.04 / +0.06 dB. **Overlap 2.**
- Karaoke (`bs_karaoke_anvuew`): **+0.53 dB at 4 and +0.72 dB at 8**, 8 of 10 tracks
  better, the largest effect in this study, on the weakest step of the chain.
  **Overlap 8.** frazer & becruily gains less (+0.18 dB at 4, +0.24 dB at 8), so it
  runs at 4; the karaoke models not swept use 8 like the default.
- On these 10 a cappellas frazer & becruily is 2.9 dB ahead of anvuew at overlap 2,
  while over all 41 the two are level (mean +0.19 dB, medians 6.62 vs 6.61 dB): per-track
  lead SDR ranges from -8 to +31 dB here, so small subsets swing a lot.
- Over the whole set (second table of 5.3, measured in the second session) anvuew gains
  +0.36 dB a cappella (30 of 41 tracks better) and +0.41 dB lead SDR in mixes (16 of 18)
  from overlap 2 to 8: half the 10-track figure, still four times the threshold. At the
  shipped overlaps anvuew (8) is ahead of frazer & becruily (4) on lead SDR in mixes by
  0.55 dB; frazer & becruily at 8 was not run on the whole set (+0.07 dB over 4 on the
  10 tracks).

The karaoke-model comparison in the first table of 5.3 and the first table of 5.4 ran
at overlap 2 for every model; the second tables of 5.3 and 5.4 and sections 6.3 to 6.6
use the defaults above.

### 6.3 "Nothing lost": the four stems add up to the original

Real separations (default models and overlaps) of four MUSDB excerpts, 60 s each, both
presets. `other` is defined as the residual, so in exact arithmetic the four stems add
up to the original; the question is what rounding and storage do to that.

**Nothing lost (lossless-ex60-n4)**

Worst track per column. Error = sum of the four stems at unity minus the original. One 24-bit LSB = 1.19e-07 (-138.5 dBFS). Four stems rounded to 24 bits independently give an error RMS of sqrt(4/12) LSB = -143.2 dBFS (quantisation level).

| preset | tracks | path | max abs error | max abs error, LSB | SNR, dB | error RMS, dBFS |
|---|---|---|---|---|---|---|
| full | 4 | float32 stems, float32 sum | 5.96e-08 | 0.50 | 156.5 | -174.0 |
| full | 4 | each stem 24-bit (writer's gain rule), float64 sum | 2.38e-07 | 2.00 | 123.5 | -143.2 |
| full | 4 | .jksong written + read back, exact residual, float64 sum | 0 (bit-exact) | 0 | exact | exact |
| full | 4 | .jksong read back as float32 stems, float32 sum | 0 (bit-exact) | 0 | exact | exact |
| clean | 4 | float32 stems, float32 sum | 5.96e-08 | 0.50 | 155.7 | -173.9 |
| clean | 4 | each stem 24-bit (writer's gain rule), float64 sum | 2.38e-07 | 2.00 | 123.5 | -143.2 |
| clean | 4 | .jksong written + read back, exact residual, float64 sum | 0 (bit-exact) | 0 | exact | exact |
| clean | 4 | .jksong read back as float32 stems, float32 sum | 0 (bit-exact) | 0 | exact | exact |

Stems that needed a gain below unity (peak above full scale): none.

- **float32:** the four float32 stems summed in float32 (as a player mixes them) differ
  from the original by at most 5.96e-8, half a 24-bit LSB, at -174 dBFS RMS: float32
  rounding only.
- **24-bit, each stem on its own:** at most 2 LSB (2.38e-7), error RMS -143.2 dBFS, SNR
  at least 123.5 dB against the original. That is exactly four independent rounding
  errors (sqrt(4/12) LSB = -143.2 dBFS): quantisation level.
- **Through a real `.jksong`** with the writer's exact-residual mode (`other` is
  computed from what the other three stems decode to): the decoded stems add up to the
  original **bit for bit** here. That is because MUSDB's mixtures are 16-bit, so every
  value is representable in 24 bits. For a float source (a decoded MP3, say) the bound
  is half an LSB of the residual stem; `tests/test_separate.py::test_song_file_round_trip_loses_nothing`
  checks that bound on random float32 audio whose stems exceed full scale, so the
  stored-gain path is exercised too (no stem in these four excerpts needed a gain).

The per-stem gain rule is the song writer's (`jksong.quantize_stem`, `docs/SONG_FORMAT.md`
4.2): gain 1.0 unless a stem would clip, then peak 0.99 and the inverse gain stored.

**A float source whose stems exceed full scale.** The run above never used the stored
gain (MUSDB's 16-bit mixtures peak below 1.0) and its source lies on the 24-bit grid. So
the same four excerpts were multiplied by 10^(9/20) (+9 dB, not a power of two: the
samples are no longer on any 24-bit grid, like a decoded MP3) and separated again with
both presets (`lossless --mix-gain-db 9`, run 15:34-15:42):

**Nothing lost, +9 dB float source (lossless-ex60-n4-g+9dB)**

| preset | tracks | path | max abs error | max abs error, unity LSB | SNR, dB | error RMS, dBFS |
|---|---|---|---|---|---|---|
| full | 4 | float32 stems, float32 sum | 2.38e-07 | 2.00 | 151.1 | -159.3 |
| full | 4 | each stem 24-bit (writer's gain rule), float64 sum | 4.38e-07 | 3.68 | 130.4 | -138.1 |
| full | 4 | .jksong written + read back, exact residual, float64 sum | 1.30e-07 | 1.09 | 134.5 | -142.5 |
| full | 4 | .jksong read back as float32 stems, float32 sum | 3.58e-07 | 3.00 | 134.3 | -142.3 |
| clean | 4 | float32 stems, float32 sum | 2.38e-07 | 2.00 | 151.3 | -159.4 |
| clean | 4 | each stem 24-bit (writer's gain rule), float64 sum | 4.24e-07 | 3.56 | 130.3 | -138.3 |
| clean | 4 | .jksong written + read back, exact residual, float64 sum | 1.24e-07 | 1.04 | 134.6 | -142.9 |
| clean | 4 | .jksong read back as float32 stems, float32 sum | 4.77e-07 | 4.00 | 134.4 | -142.7 |

Mixture peaks 2.32 to 2.77. 26 of the 32 stems needed a stored gain (+0.13 to +7.81 dB:
drums and "other" on every track, the lead on 3 of 4, backing on 2 of 4).

- **float32:** at most 2.38e-7, which is one float32 step (ulp) for values between 2 and 4:
  float32 rounding of signals that now reach 2.8, nothing else (SNR >= 151 dB).
- **24-bit, each stem on its own:** at most 4.4e-7, error RMS -138.1 dBFS, SNR >= 130.3 dB.
  A stem stored with gain g has a 24-bit step of g x LSB (up to 2.5 LSB here), so four
  such roundings land 2-5 dB above the unity-gain figure of -143.2 dBFS: quantisation
  level for the gains used.
- **Through a real `.jksong`** (exact residual): at most 1.30e-7, and on **8 of 8** tracks
  within the bound of half a step of the residual stem (half an LSB x its stored gain;
  over 2.6 million samples the error reaches that bound to 6 digits on every track and
  never exceeds it); SNR >= 134.5 dB, error RMS
  -142.5 dBFS. Read back as float32 stems and summed in float32, as the player mixes, the
  error is at most 4.8e-7 (two float32 steps at this level), SNR >= 134.3 dB.

### 6.4 48 kHz sources

Every model here runs at 44.1 kHz. For any other source rate `Separator.run_model`
resamples only the model's input down (soxr VHQ, float64 inside) and its estimate back
up; the original is never resampled, and the residual stems are computed from the
original at its own rate. Test: four MUSDB excerpts converted to 48 kHz (and their true
vocals with them) run through the default `full` preset.

**48 kHz source**

| tracks | vocals SDR, native 44.1 kHz | vocals SDR, 48 kHz path | worst abs difference | stems at 48 kHz, input length | stems sum, max abs error |
|---|---|---|---|---|---|
| 4 | 14.940 | 14.931 | 0.028 dB | True | 2.98e-08 |

- The vocal SDR through the 48 kHz path is within 0.03 dB of the native 44.1 kHz run
  (worst track; the others within 0.005 dB). All three model runs were flagged as
  resampled, the stems came back at 48 kHz with exactly the input's length, and the four
  stems add up to the 48 kHz original to 3e-8 (float32 rounding).
- What the conversion costs, measured with sine tones sent 48 -> 44.1 -> 48 kHz: level
  flat to -0.01 dB up to 20 kHz (error -154 dB at 1 kHz, -90 dB at 16 kHz, -60 dB at 20
  kHz), -0.07 dB at 20.5 kHz, -8 dB at 21 kHz, gone above 21.5 kHz. So the models never
  see content above about 20.5 kHz. In the `full` preset that content stays in `other`
  (the residual), so the karaoke mix keeps it; in `clean`, whatever the instrumental
  model cannot see ends up in the lead stem, i.e. leaves the karaoke mix. One more small
  reason to prefer `full` for 48 kHz and higher sources.

### 6.5 Speed per model

**Model speed: GPU seconds per minute of audio, one 60 s clip (`AM Contra - Heart Peripheral`)**

Each model alone on the GPU, fp32, median of 3 runs after a warm-up. GPU at the start of each run: 2026-11-17 10:15: utilisation, memory used = 0 %, 1620 MiB; 2026-11-17 11:11: utilisation, memory used = 0 %, 1830 MiB.

| model | role | overlap | GPU s per audio minute | 4-minute song, s | peak VRAM MB |
|---|---|---|---|---|---|
| `bs_roformer_anvuew_ft1` | vocals | 2 | 10.18 | 41 | 2992 |
| `bs_roformer_leap_xe_vocals` | vocals | 2 | 20.38 | 82 | 3918 |
| `mel_roformer_kim` | vocals | 2 | 5.47 | 22 | 2336 |
| `mel_roformer_duality` | vocals | 2 | 5.95 | 24 | 3516 |
| `htdemucs_ft_vocals` | vocals | 2 | 0.84 | 3 | 1106 |
| `bs_karaoke_anvuew` | karaoke | 2 | 7.57 | 30 | 2080 |
| `bs_karaoke_frazer_becruily` | karaoke | 2 | 8.33 | 33 | 2766 |
| `mel_karaoke_aufr33_viperx` | karaoke | 2 | 5.58 | 22 | 2336 |
| `bs_karaoke_gabox_is` | karaoke | 2 | 6.43 | 26 | 1284 |
| `mel_karaoke_becruily` | karaoke | 2 | 5.52 | 22 | 3636 |
| `mel_inst_becruily` | instrumental | 2 | 4.77 | 19 | 2336 |
| `mel_inst_unwa_v1e_plus` | instrumental | 2 | 5.19 | 21 | 2820 |
| `bs_inst_resurrection_unwa` | instrumental | 2 | 9.60 | 38 | 2728 |
| `htdemucs_ft_drums` | multistem | 2 | 0.82 | 3 | 1106 |
| `bs_roformer_4stems_syh` | multistem | 2 | 6.76 | 27 | 2490 |
| `scnet_xl_ihf_zfturbo` | multistem | 2 | 8.53 | 34 | 4678 |
| `bs_roformer_viperx_1297` | vocals | 2 | 10.86 | 43 | 2590 |
| `mel_roformer_becruily_vocals` | vocals | 2 | 4.66 | 19 | 2336 |

All at overlap 2; the time grows about linearly with the overlap (section 6.2), so the
default karaoke model at its default overlap 8 needs about 34 s per minute of audio.

**Batching chunks does not help.** Several chunks per network call (`batch_size`), the
default karaoke model at overlap 8, same 60 s clip, fp32, best of 2 runs, measured twice
(16:42 and 16:47, GPU idle; `separation_bench.py batch`): batch 1 30.60 / 31.20 s per
audio minute (2080 MB), batch 2 30.87 / 31.20 s (3888 MB), batch 4 30.14 / 30.49 s
(7480 MB). One 14.5 s chunk already saturates the GPU: batching saves at most 2 %,
multiplies the VRAM by up to 3.6 and changes the output by -122 dB (max abs 8.5e-7,
different kernel choices), so the Studio keeps batch 1.

### 6.6 Whole separation of a 4-minute song

**Whole separation of one song: `Side Effects Project - Sing With Me` (244 s, stereo, 44.1 kHz, overlap per model)**

Two runs of `timing --overlap 0` (each model at its default overlap, as the app runs):

| run | preset | separation wall time, s | model loading, s | per model, GPU s | peak VRAM MB |
|---|---|---|---|---|---|
| 11:38 | full | 159.6 | 21.9 (weights not in the OS file cache) | `bs_roformer_anvuew_ft1` 28.3 (overlap 2), `bs_karaoke_anvuew` 103.8 (overlap 8), `bs_roformer_4stems_syh` 26.9 (overlap 2) | 4128 |
| 11:38 | clean | 177.0 | 26.6 | `bs_roformer_anvuew_ft1` 27.1, `bs_karaoke_anvuew` 96.0, `bs_inst_resurrection_unwa` 29.1, `bs_roformer_4stems_syh` 24.3 | 4326 |
| 15:20 | full | **151.8** | 1.9 (weights in the OS file cache) | `bs_roformer_anvuew_ft1` 27.8 (overlap 2), `bs_karaoke_anvuew` 99.1 (overlap 8), `bs_roformer_4stems_syh` 24.7 (overlap 2) | 4128 |
| 15:20 | clean | 180.7 | 1.8 | `bs_roformer_anvuew_ft1` 28.1, `bs_karaoke_anvuew` 97.0, `bs_inst_resurrection_unwa` 29.7, `bs_roformer_4stems_syh` 24.9 | 4326 |

GPU at the start: 11:38 1 %, 2003 MiB in use; 15:20 6 %, 4702 MiB (read one second after
the previous benchmark process exited; 2540 MiB after the run). In the 15:20 run a
23-second CPU test suite ran alongside for part of the `full` pass; every model time
of the two runs agrees within 10 % (within 4 % in the `clean` pass).

**A 4:04 stereo song through the default `full` preset takes 152-160 s of separation**
(vocals 28 s, karaoke 99-104 s at overlap 8, drums 25-27 s), plus 2 s (weights already
in the OS file cache) to 22 s (cold) to load the three models onto the GPU the first
time; they stay loaded for the next song in a queue. That is about 3 minutes for the
first song and 2.5-2.7 minutes for each further one, with 4.1 GB of VRAM at peak. The
`clean` preset adds the instrumental model: 177-181 s. Wall time equals the sum of the
model times: the float64 arithmetic around the models is negligible. Lowering the
karaoke overlap to 2 would save about 75 s per song and cost 0.7 dB of lead SDR
(section 6.2); it is not done.

### 6.7 Network code and determinism

- **Parity with upstream.** For every RoFormer and SCNet checkpoint in the registry (16
  models), `roformer.py` / `scnet.py` and the upstream MSST network code
  (ZFTurbo/Music-Source-Separation-Training at commit `1f700c38`, 2026-11-12) were run on
  the same 3 s random stereo input with the same weights on the CPU in fp32: **max abs
  difference 0, bit-identical outputs, zero missing or unexpected parameters** for all
  16. HT-Demucs runs through the official `demucs` 4.1.0 package.
- **Determinism.** The default vocal model re-run in the afternoon on two MUSDB excerpts
  reproduced the morning's cached estimates bit for bit (max abs difference 0.0), and
  the GPU test `test_full_preset_runs_fp32_sums_back_and_is_repeatable` runs the whole
  default preset twice and requires identical stems.
- **No 16-bit step anywhere.** The separation API works on float32 arrays end to end;
  `python-audio-separator`'s file writers (16-bit WAV by default) are not used. The only
  integer step is the song writer's 24-bit FLAC (section 6.3).

### 6.8 Re-verification in a second session (15:19 onwards)

Re-run from scratch, not read back from the morning's files:

| check | result |
|---|---|
| `pytest -m gpu tests/test_gpu.py` (default preset twice: identical stems, sum back to the mix; 48 kHz input; all 18 checkpoints load) | 20 passed, 42.7 s |
| Default vocal model on 2 MUSDB excerpts vs the estimates cached at 07:17 | max abs difference **0** (bit-identical); SDR 15.1216 / 11.1922 dB |
| `lossless --limit 4` (section 6.3, unity) | every value identical to the 12:01 run |
| `resample48 --limit 4` (section 6.4) | identical SDRs (15.121198 dB etc.), 48 kHz output, sum error 2.98e-8 |
| `timing` (section 6.6) | `full` 151.8 s (159.6 s at 11:38), VRAM 4128 MB both times |
| First-use download into an empty folder (`python -I -m jukeoke_studio --models-dir <empty> models fetch htdemucs_ft_drums bs_karaoke_anvuew`) | 84 MB in 1.5 s, 204 MB in 3.8 s from the pinned URLs; SHA-256 verified while streaming, confirmed independently with `sha256sum`; `manifest.json` written |
| Licence tags and revisions of the default models (Hugging Face API) | `anvuew/BS-RoFormer` gpl-3.0, `anvuew/karaoke_bs_roformer` gpl-3.0, `SYH99999/bs_roformer_4stems_ft` apache-2.0; the pinned revisions are still each repository's current head |
| CPU tests, whole `studio/tests` (after `pyproject.toml` gained the lyrics/alignment dependencies) | 145 passed, 1 skipped (a network test), 20 GPU tests deselected |

## 7. Recommendations

| | choice | licence (commercial use) | why |
|---|---|---|---|
| Vocals | `bs_roformer_anvuew_ft1` | GPL-3.0 (yes) | 0.23 dB behind the best open model, which has no licence |
| Lead vs backing | `bs_karaoke_anvuew` (overlap 8) | GPL-3.0 (yes) | best lead SDR in mixes (10.78 dB) and karaoke-mix SDR (18.10 dB) at the shipped settings; the best unlicensed one (frazer & becruily) leaves 1 dB less lead residue |
| Drums | `bs_roformer_4stems_syh` | Apache-2.0 (yes) | +1.1 dB over HT-Demucs; SCNet XL (+0.8 dB) has non-commercial training data |
| Default karaoke mix | **"full"**: original minus estimated lead | - | higher SDR and less ghost vocal than any direct-instrumental model |
| "clean" preset | `bs_inst_resurrection_unwa` instead of `mix - vocals` | none published (unclear) | best direct-instrumental model; keeps cymbals brighter, more residue |
| Precision | fp32, TF32 off | - | fp16 is 2x faster but not identical (-65 dB) |
| Overlap | 2 for vocals, drums, instrumental; 8 for the karaoke model | - | karaoke +0.41 dB lead SDR in mixes at 8 (whole set); the others gain under 0.1 dB (section 6.2) |

**Permissively licensed alternatives.** The default set already allows commercial use
(two GPL-3.0 models and one Apache-2.0). For a set with no GPL weights at all: vocals
`mel_roformer_kim` (MIT, -0.63 dB) or `mel_roformer_duality` (Apache-2.0, -0.74 dB),
drums `bs_roformer_4stems_syh` (Apache-2.0) or `htdemucs_ft_drums` (MIT, -1.1 dB). No
MIT/Apache karaoke model of comparable quality exists: the only one found
(musetric's Apache-2.0 duality-kara2 student, trained on Creative Commons data only)
reports 6.2 dB lead SDR on ccMixter against 10.8 dB here. There is no permissively
licensed direct-instrumental model of competitive quality either, so "clean" has no
permissive version.

**Best measured regardless of licence** (`separate.BEST_MEASURED`, for listening
comparisons only): Leap Xe vocals, frazer & becruily karaoke, SCNet XL drums,
Resurrection instrumental. All four publish no licence. For karaoke this is a trade, not
a clear win (section 5.3): less lead residue and a better backing stem, 0.55 dB lower
lead SDR at the shipped overlaps.

## 8. What only ears can settle (for the owner's listening checklist)

- Default ("full") against "clean" on the same song, backing vocals on: is the ghost of
  the lead singer audible in quiet passages, and do the cymbals of "full" sound duller?
  (Measured at the shipped settings: full has 1.1 dB less lead residue; clean keeps
  0.3 dB more at 4-8 kHz and 0.6 dB more at 8-16 kHz.)
- Backing vocals on versus off: does lead residue sit in the backing stem (measured:
  the lead/backing split is the weakest step, lead SDR about 10.8 dB)?
- Songs with doubled or harmonised lead vocals and duets, where "lead" is ambiguous.
- `BEST_MEASURED` against the default on two or three songs, to decide whether asking
  the authors of the unlicensed models for permission is worth it. For the karaoke model
  in particular: frazer & becruily leaves about 1 dB less lead in the karaoke mix and
  keeps backing vocals better, the default scores 0.55 dB higher on lead SDR; only ears
  can say which matters more.

## 9. Reproduce

From the repository root in Git Bash, after `source scripts/devenv.sh`
(`PY=$JK_BUILD/studio-venv/Scripts/python.exe`):

```
$PY -I studio/bench/fetch_eval.py musdb    --out "$JK_BUILD/eval/musdb18hq"         # 7.9 GB
$PY -I studio/bench/fetch_eval.py ccmixter --out "$JK_BUILD/eval/ccmixter_voices"   # 1.5 GB
B=studio/bench/separation_bench.py
# quality (sections 5.1-5.5)
$PY -I $B musdb --cache --models bs_roformer_anvuew_ft1                     # 50 tracks
$PY -I $B musdb --cache --from-cache --limit 25 --models bs_roformer_leap_xe_vocals
$PY -I $B musdb --limit 25 --models mel_roformer_kim,mel_roformer_duality,htdemucs_ft_vocals
$PY -I $B musdb --limit 25 --models bs_roformer_viperx_1297,mel_roformer_becruily_vocals
$PY -I $B musdb --limit 25 --models mel_inst_becruily,mel_inst_unwa_v1e_plus,bs_inst_resurrection_unwa
$PY -I $B karaoke --excerpt 45 --vocals-model bs_roformer_anvuew_ft1 \
    --models bs_karaoke_anvuew,bs_karaoke_frazer_becruily,mel_karaoke_aufr33_viperx,bs_karaoke_gabox_is,mel_karaoke_becruily
$PY -I $B presets --excerpt 45 --models mel_inst_becruily,mel_inst_unwa_v1e_plus,bs_inst_resurrection_unwa
$PY -I $B drums --limit 25 --vocals-model bs_roformer_anvuew_ft1 \
    --models htdemucs_ft_drums,scnet_xl_ihf_zfturbo,bs_roformer_4stems_syh
# engineering checks (section 6)
$PY -I $B precision --model bs_roformer_anvuew_ft1 --limit 4
$PY -I $B overlap --model bs_roformer_anvuew_ft1 --limit 6
$PY -I $B overlap --model bs_roformer_4stems_syh --limit 6
$PY -I $B overlap --model bs_inst_resurrection_unwa --limit 6
$PY -I $B overlap --model bs_karaoke_anvuew --excerpt 45 --limit 10
$PY -I $B overlap --model bs_roformer_4stems_syh --limit 12 --overlaps 2,4,8
$PY -I $B overlap --model bs_roformer_anvuew_ft1 --limit 12 --overlaps 2,4,8
$PY -I $B overlap --model bs_karaoke_frazer_becruily --excerpt 45 --limit 10 --overlaps 2,4,8
$PY -I $B presets --excerpt 45 --overlap 0 --models bs_inst_resurrection_unwa
$PY -I $B speed --overlap 2 --models <every model id, comma-separated>
$PY -I $B batch --model bs_karaoke_anvuew --overlap 0
$PY -I $B timing --overlap 0          # 0 = each model's default overlap, as the app runs
$PY -I $B lossless --limit 4 --overlap 0
$PY -I $B lossless --limit 4 --overlap 0 --mix-gain-db 9   # float source, stored gains
$PY -I $B resample48 --limit 4 --overlap 0
$PY -I $B karaoke --excerpt 45 --overlap 0 --models bs_karaoke_anvuew,bs_karaoke_frazer_becruily
$PY -I $B report > tables.md
```

Model weights download on first use (about 10 GB for every candidate; the default
`full` preset needs 0.94 GB). Unit tests: `cd studio && $PY -I -m pytest tests/test_separate.py
tests/test_roformer.py tests/test_quality.py tests/test_registry.py` (CPU, no weights) and
`$PY -I -m pytest -m gpu tests/test_gpu.py` (real models on the GPU).

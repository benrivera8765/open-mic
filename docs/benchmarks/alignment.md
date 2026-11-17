# Lyrics alignment benchmark (Studio steps 3-4)

Status: **measured on this machine, 17 November 2026.** Every number below was produced
by `studio/bench/alignment_bench.py` on the RTX 5070 Ti. Raw per-song results (JSON) are
in `$JK_BUILD/eval/jamendolyrics/_work/` (outside the repository).

## Summary

Word-onset error |predicted start - annotated start| on the 20 English JamendoLyrics
songs (5,693 words), aligning against the **lead vocal separated by the Studio itself**,
default model (`wav2vec2-large-960h-lv60-self`), default settings:

| Mode | Mean | Median | p90 | p95 | Worst | Within 0.3 s |
|---|---|---|---|---|---|---|
| (a) Text only | 0.209 s | **0.036 s** | 0.134 s | 0.468 s | 54.4 s | **94.2 %** |
| (b) With line anchors: dataset line starts as LRC-style windows (±1 s) | 0.120 s | **0.036 s** | 0.129 s | 0.361 s | 25.5 s | **94.7 %** |
| (b') Same, line starts jittered by N(0, 0.3 s) like a sloppy LRC | 0.118 s | 0.036 s | 0.129 s | 0.353 s | 25.5 s | 94.8 % |
| (b'') Same, every line start 2 s late (LRC for another edit; corrected automatically, section 5.4) | 0.120 s | 0.036 s | 0.129 s | 0.361 s | 25.5 s | 94.7 % |
| (c) Line starts as hard anchors (protocol `realign`; first words of lines excluded, 4,825 words) | 0.102 s | 0.035 s | 0.125 s | 0.237 s | 25.5 s | 95.5 % |
| For comparison: text only on the **full mix** (no separation) | 3.525 s | 0.048 s | 11.3 s | 19.0 s | 81.0 s | 76.3 % |

* **The median is 0.036 s**, well under the 0.1 s the WORK_ORDER asks to investigate;
  the mean and the worst cases are set by a few songs (section 6).
* **Held notes:** word *end* error with the voicing-gated extension is median 0.050 s,
  p90 0.233 s; without it (letters end where CTC stops) median 0.101 s, p90 0.417 s;
  extending through everything up to the next word, median 0.053 s but p90 0.582 s and
  p95 1.38 s (holds run into the silence). Section 5.2.
* **Separation matters:** on the full mix 24 % of words are more than 0.3 s off
  (mean 3.5 s); on the separated lead vocal 5.8 %.
* **Time per song on the GPU** (default model, 3-4 min songs, GPU shared): 1.6-2.2 s for
  the whole alignment (1.4-1.9 s model, 0.2-0.3 s Viterbi and letter timing), model
  load 1.5 s, peak VRAM 2.1 GB.
* **Dev/test:** settings were tuned on the first 10 songs only; the other 10 ("test")
  give median 0.036 s (text), 0.036 s (lines), 0.034 s (hard), section 5.1.
* **Transcription** (no lyrics given): Whisper large-v3-turbo on the separated lead
  vocal, see section 7 for word error rates and speed.
* **Not measured here:** letter-level accuracy (no dataset has letter times; letters are
  checked by synthetic tests in `studio/tests/test_align_ctc.py`), languages other than
  English, real LRC files from LRCLIB (approximated by the jittered line starts).

## 1. Set-up

| | |
|---|---|
| GPU | NVIDIA GeForce RTX 5070 Ti, 16 GB (sm_120) |
| Software | Python 3.12.16, PyTorch 2.14.1+cu130, transformers 5.23.0, faster-whisper 1.2.1 / CTranslate2 4.9.0, numpy 2.5.5 |
| Precision | Alignment models fp32; Whisper fp16 (OpenAI's own reference precision) |
| Data | JamendoLyrics, English subset: 20 songs, 5,693 annotated words (details in section 4) |
| Separation | The Studio's own `jukeoke_studio/separate.py` (separation workstream), `full` preset models: BS-RoFormer ft1 (anvuew) for vocals, then BS-RoFormer Karaoke (anvuew) for the lead vocal. The aligner sees only the lead vocal, as in the real pipeline. |

**GPU sharing.** The separation workstream's benchmark ran on the same card for the whole
session (99 % utilisation, 10-15 GB VRAM in use by other processes). Accuracy numbers are
unaffected; every time below is an upper bound.

## 2. Options and licences (research, November 2026)

### 2.1 Transcription of sung vocals

| Option | Code licence | Weights licence | Commercial use | Notes |
|---|---|---|---|---|
| Whisper large-v3 via faster-whisper | faster-whisper MIT, CTranslate2 MIT | OpenAI Whisper: MIT (GitHub); `openai/whisper-large-v3` HF card says Apache-2.0; CT2 conversion `Systran/faster-whisper-large-v3`: MIT | yes | 3.09 GB |
| Whisper large-v3-turbo via faster-whisper | MIT | `openai/whisper-large-v3-turbo`: MIT; CT2 conversion `mobiuslabsgmbh/faster-whisper-large-v3-turbo`: MIT | yes | 1.62 GB, 4 decoder layers instead of 32 |
| Whisper via HF transformers on PyTorch | Apache-2.0 | as above | yes | would run on torch cu130 without extra CUDA libraries; not measured; kept as the fallback if the extra 737 MB of CUDA 12 cuBLAS (below) is unwanted |

**CTranslate2 on the RTX 5070 Ti (sm_120): works, with one extra package.** The
CTranslate2 4.9.0 wheel is a CUDA 12 build. It asks for `cublas64_12.dll`, but the
PyTorch cu130 wheel only ships CUDA 13 libraries, so the first test failed with
`RuntimeError: Library cublas64_12.dll is not found or cannot be loaded`. Installing
`nvidia-cublas-cu12==12.9.2.10` (737 MB on disk) and preloading its two DLLs with
`ctypes` (CTranslate2 loads them by bare name, so `os.add_dll_directory` is not enough;
see `transcribe.add_cuda12_dll_dirs`) fixed it. cuDNN came from the copy CTranslate2
ships plus PyTorch's; `nvidia-cudnn-cu12` was not needed and was removed again. Then
large-v3 and large-v3-turbo ran in fp16 on the GPU (kernels for sm_120 present).
Packaging note: `ctranslate2.dll` imports `MSVCP140.dll` / `VCRUNTIME140(_1).dll`, which
must be bundled with the portable Studio runtime.

### 2.2 Character-level CTC models for forced alignment (English)

| Model | Licence (weights) | Commercial | Size | Used |
|---|---|---|---|---|
| `facebook/wav2vec2-large-960h-lv60-self` (same family as WhisperX's optional `WAV2VEC2_ASR_LARGE_LV60K_960H`) | Apache-2.0 | yes | 1.26 GB | **default** |
| `facebook/hubert-xlarge-ls960-ft` (HuBERT X-Large, LibriSpeech 960 h, characters) | Apache-2.0 | yes | 3.85 GB | measured, option (`Aligner(model_name=...)`) |
| `facebook/wav2vec2-base-960h` (WhisperX's English default via torchaudio) | Apache-2.0 | yes | 0.38 GB | not measured |
| `facebook/data2vec-audio-large-960h` | Apache-2.0 | yes | 1.26 GB (`.bin` only) | not measured |
| MMS forced aligner (`MahmoudAshraf/mms-300m-1130-forced-aligner`, torchaudio `MMS_FA`), `facebook/mms-300m` | **CC-BY-NC-4.0** | **no** | | excluded |

All Apache-2.0 models were downloaded from huggingface.co at pinned revisions
(`align.MODEL_INFO`). The torchaudio forced-alignment API (`torchaudio.functional.forced_align`)
is deprecated, so the trellis and Viterbi backtrack are our own (`align.ctc_viterbi`).

### 2.3 Prior art read

* **WhisperX** (BSD-2-Clause): transcribe with Whisper on VAD segments, then CTC forced
  alignment of each segment's transcript with a wav2vec2 model per language (English:
  torchaudio `WAV2VEC2_ASR_BASE_960H`; `WAV2VEC2_ASR_LARGE_LV60K_960H` optional). Words
  with characters outside the model's dictionary (numbers, symbols) get no time at all.
* **nomadkaraoke `python-lyrics-transcriber`** (MIT, archived, merged into `karaoke-gen`):
  transcribes with Whisper (whisper-timestamped) or the commercial AudioShake API, fetches
  reference lyrics (Genius, Spotify, Musixmatch, LRCLIB), finds *anchor sequences*
  (n-gram matches of at least 3 words between transcript and references, longest
  first, each position used once), then corrects the gaps between anchors (rules and an
  optional LLM). Word times come from the transcription. **`karaoke-gen`** (MIT) wraps it
  with MDX/Demucs separation, a human review UI and video rendering.
* **UltraSinger** (MIT): Demucs htdemucs separation first ("the vocals are separated
  from the audio before they are passed to the models"), Whisper/WhisperX for text and
  word times, SwiftF0 for pitch, hyphenation for syllables (split without checking where
  they are sung).

What we took: align against the separated lead vocal (all three); anchor *runs* of at
least three matched words (lyrics-transcriber) for the optional Whisper anchors; a
global CTC alignment with our own letters-to-graphemes mapping instead of per-segment
alignment, so that every grapheme gets a time (WhisperX leaves numbers untimed).

## 3. Method (`studio/jukeoke_studio/align.py`)

1. **Emissions.** Lead-vocal stem -> mono -> 16 kHz (soxr VHQ, a copy for the model
   only) -> standardised with the mean/std of the louder half of the song (so silent
   passages stay silent instead of being blown up per chunk) -> CTC model in 30 s chunks
   with 2 s of context on each side -> log-probabilities per 20 ms frame.
2. **Tokens.** Every grapheme cluster maps to the model's letters: accents stripped
   (é -> E), `ß`/`æ`/`œ` expanded, `&` -> AND, `%` -> PERCENT, ASCII digit runs spelled
   the way they are usually sung (`1999` -> NINETEEN NINETY NINE, `2005` -> TWO THOUSAND
   FIVE, `42` -> FORTY TWO), word-internal apostrophes kept. Graphemes with no letter
   (punctuation, symbols, emoji, other scripts) get no token. The word separator `|` is
   merged into the CTC blank, so word boundaries are optional.
3. **Constrained CTC Viterbi** (`ctc_viterbi`): one global monotonic alignment over the
   whole song (standard 2N+1 blank/letter states, skip rule for repeated letters), in
   numpy, banded: each letter may be restricted to a frame window.
   * *Hard anchors* (protocol `realign`, line starts fixed by the user): the line's first
     letter starts exactly at the anchor frame (the blank before it is capped too);
     everything before ends before it. The output line start is the anchor time to the
     millisecond.
   * *LRC / online line times*: each line's letters must lie in
     [LRC start - 1 s, next LRC time + 1 s]. Empty timed LRC lines (instrumental breaks)
     count as "next LRC time". Before that, an unconstrained pass measures the median
     offset between each line's sung start and its LRC time; if it is at least 0.3 s and
     at least half of the lines agree with it within 0.5 s, all LRC times are moved by it
     and a warning says so (an LRC made for another edit of the song). Section 5.4.
   * *Silence*: letters in frames more than 50 dB below the vocal stem's 99th-percentile
     level cost 4 nats each, so lyrics are not pulled into instrumental breaks.
   * If the windows cannot be met (bad LRC), the aligner relaxes to the next level
     (windows -> anchors only -> nothing) and reports a warning.
4. **Letter times.** A letter starts at its first CTC frame. Inside a word it ends where
   the next letter starts. The last letter of a word is **extended through the
   following blank frames while the vocal stem is voiced** (within 15 dB of the robust
   peak, dips of up to 3 frames bridged) and never past the next word: a held note covers
   the hold, but not the silence after it. Untokenised graphemes are interpolated inside
   their word: punctuation gets zero length at its place, sung graphemes without a
   letter (other scripts) share the neighbouring letter's span; a word with no letter at
   all is placed in the gap between its neighbours (at most 0.3 s per grapheme).
5. **Calibration.** CTC fires slightly after the annotated onset; a per-model constant
   (`MODEL_INFO[...]["time_shift_s"]`, -0.03 s for HuBERT, -0.04 s for wav2vec2) is
   added to every time. It was measured on the dev songs only (section 5).
6. Output: the lyrics JSON of SONG_FORMAT.md section 5, with
   `alignment = {"method": "ctc-forced", "model": ..., "anchored_lines": ..., "constraints": "none|lines|words"}`.

Optional (off by default, see section 6): coarse anchors from a Whisper transcript,
matched to the lyrics by Needleman-Wunsch on words, runs of at least 3 matches.

## 4. Data and protocol

* **JamendoLyrics** (Durand, Stoller & Ewert, ICASSP 2023; English part = the songs of
  Stoller et al., ICASSP 2019), cloned from https://github.com/f90/jamendolyrics (the
  repository says the maintained copy is now on Hugging Face, `jamendolyrics/jamendolyrics`;
  the GitHub copy is the one named in the work order) into
  `$JK_BUILD/eval/jamendolyrics`. Code and annotations: MIT. Audio and lyrics: Creative
  Commons, per song (column `LicenseType` of `JamendoLyrics.csv`): BY, BY-SA, BY-ND,
  BY-NC, BY-NC-SA, BY-NC-ND. Used for evaluation only; nothing from it is in the repository.
* English subset: 20 songs, 72 minutes, 5,693 words with manual word start/end times;
  line times in `annotations/lines` are derived from the word times.
* Lyrics text = `lyrics/<song>.txt` (lower case, normalised by the dataset), read with
  the Studio's own `lyrics.from_text`; the word count matches the annotation for every
  song (checked by the bench).
* Lead vocal: `separate.Separator.run_model` with the `full` preset's vocal model then
  its karaoke model (lead), at the file's native rate, fp32. 20 songs took 79-1,085 s
  each (median 187 s) because the separation workstream's MUSDB benchmark was running on
  the same GPU; those times say nothing about the separator's real speed.
* Metric: word-onset absolute error, pooled over all words; "AAE" (per-song mean,
  averaged over songs, the MIREX measure) is in the JSON. Dev = first 10 English songs
  in CSV order (all tuning), test = the other 10.

## 5. Results

### 5.1 Word onsets: models, modes, dev/test

Pooled over songs; seconds. Default model and settings unless the row says otherwise.

| Model | Mode | Songs | Words | Mean | Median | p90 | p95 | Worst | Within 0.3 s | AAE |
|---|---|---|---|---|---|---|---|---|---|---|
| wav2vec2-large | text | all | 5,693 | 0.209 | 0.036 | 0.134 | 0.468 | 54.38 | 94.2 % | 0.235 |
| wav2vec2-large | text | dev | 2,792 | 0.137 | 0.036 | 0.126 | 0.207 | 30.21 | 95.9 % | 0.161 |
| wav2vec2-large | text | test | 2,901 | 0.279 | 0.036 | 0.150 | 0.760 | 54.38 | 92.7 % | 0.310 |
| wav2vec2-large | lines ±1 s | all | 5,693 | 0.120 | 0.036 | 0.129 | 0.361 | 25.52 | 94.7 % | 0.121 |
| wav2vec2-large | lines ±1 s | dev | 2,792 | 0.097 | 0.036 | 0.125 | 0.200 | 7.61 | 96.1 % | 0.110 |
| wav2vec2-large | lines ±1 s | test | 2,901 | 0.142 | 0.036 | 0.144 | 0.592 | 25.52 | 93.4 % | 0.133 |
| wav2vec2-large | lines, jitter 0.3 s | all | 5,693 | 0.118 | 0.036 | 0.129 | 0.353 | 25.52 | 94.8 % | 0.121 |
| wav2vec2-large | hard anchors | all | 4,825 | 0.102 | 0.035 | 0.125 | 0.237 | 25.52 | 95.5 % | 0.107 |
| wav2vec2-large | hard anchors | test | 2,426 | 0.130 | 0.034 | 0.132 | 0.422 | 25.52 | 94.2 % | 0.125 |
| wav2vec2-large | text, full mix | all | 5,693 | 3.525 | 0.048 | 11.277 | 18.958 | 81.00 | 76.3 % | 5.104 |
| HuBERT-xlarge | text | all | 5,693 | 0.186 | 0.036 | 0.140 | 0.433 | 41.33 | 94.2 % | 0.211 |
| HuBERT-xlarge | text | test | 2,901 | 0.262 | 0.036 | 0.154 | 0.725 | 41.33 | 92.6 % | 0.301 |
| HuBERT-xlarge | lines ±1 s | all | 5,693 | 0.120 | 0.036 | 0.139 | 0.356 | 25.08 | 94.5 % | 0.132 |
| HuBERT-xlarge | lines ±1 s | test | 2,901 | 0.149 | 0.036 | 0.155 | 0.624 | 25.08 | 93.0 % | 0.163 |
| HuBERT-xlarge | hard anchors | all | 4,825 | 0.106 | 0.035 | 0.131 | 0.276 | 25.08 | 95.2 % | 0.119 |
| HuBERT-xlarge | text, full mix | all | 5,693 | 2.727 | 0.047 | 4.008 | 17.112 | 73.56 | 79.5 % | 4.087 |

**Model choice.** On the 8 dev songs available first, HuBERT X-Large looked clearly better
(mean 0.137 vs 0.186 s, median 0.043 vs 0.055 s before calibration). On all 20 songs the
two are within noise: same median and share within 0.3 s, HuBERT has a lower mean in
text-only mode (0.186 vs 0.209 s), wav2vec2 is as good or better with line windows and
anchors. HuBERT costs 3.85 GB on the stick against 1.26 GB, and 2.3x the GPU time and
VRAM. With "close in quality" the smaller model is the default; HuBERT stays selectable.
(Storing HuBERT's weights as fp16 and running in fp32 changed no metric at all: max
probability difference 0.0002, 99.99 % identical frame argmax, on 8 songs. That would
halve its size if it is ever made the default.)

### 5.2 Word ends and held notes

Word *end* error |predicted end - annotated end|, all 20 songs, text mode, default model:

| Letter end rule | Mean | Median | p90 | p95 | Within 0.3 s | Median bias |
|---|---|---|---|---|---|---|
| **Extend while the vocal stem is within 15 dB of its peak (default)** | **0.255** | **0.050** | **0.233** | **0.616** | **91.8 %** | -0.006 |
| No extension (letter ends where CTC leaves it) | 0.322 | 0.101 | 0.417 | 0.886 | 86.0 % | -0.094 |
| Extend through everything up to the next word | 0.404 | 0.053 | 0.582 | 1.379 | 84.5 % | +0.030 |
| Default, with line windows | 0.167 | 0.050 | 0.224 | 0.535 | 92.3 % | -0.004 |
| Default, with hard anchors | 0.153 | 0.050 | 0.212 | 0.434 | 93.0 % | -0.008 |

Voicing threshold chosen on the dev songs (HuBERT): p90 of the end error 0.239 s at
-10 dB, 0.223 s at -15 dB, 0.272 s at -20 dB, 0.313 s at -25 dB, 0.341 s at -30 dB.
A synthetic test (`test_held_note_extends_through_hold_not_silence`) checks the rule
exactly: a 2 s held "o" ends at the end of the hold (±0.05 s), not at the next word 2 s
later, and a 40 ms dip inside the hold is bridged.

### 5.3 Speed (RTX 5070 Ti, GPU shared with another benchmark)

| Model | Load | Peak VRAM | Per 3-4 min song: model | Viterbi + letters | Total |
|---|---|---|---|---|---|
| wav2vec2-large (default) | 1.5 s | 2.1 GB | 1.4-1.9 s | 0.2-0.3 s | 1.6-2.2 s |
| HuBERT-xlarge | 5.8 s | 4.75 GB | 4.3-4.8 s | 0.1-0.3 s | 4.5-5.1 s |

Five songs each, `Aligner.align` wall time including resampling and voicing analysis.
The Viterbi runs on the CPU in numpy (about 12,000 frames x 6,000 states per song).

Re-measured by the independent verification on 17 November 2026 with the GPU idle
(7 songs, `Aligner.align` from the lead stem, emissions in fp32): 0.91-1.46 s per song,
`torch.cuda.max_memory_allocated` 1.97 GB.

### 5.4 LRC line times with a constant offset (added by the verification)

An LRC from another edit of the song (a longer intro, a radio edit) is off by the same
amount on every line. Line windows of ±1 s in the wrong place force every line into
the wrong place, which is far worse than no windows. Measured with the dataset line
starts all moved by the same amount (all 20 songs, word onsets):

| LRC offset | Before the offset check: median / within 0.3 s | With the offset check (default): median / within 0.3 s |
|---|---|---|
| 0 s | 0.036 s / 94.7 % | 0.036 s / 94.7 % (no shift applied on any song) |
| +0.5 s / -0.5 s / +1 s | 0.036 / 94.8 %, 0.036 / 94.8 %, 0.035 / 95.0 % | 0.036 s / 94.7 % |
| +2 s | **1.886 s / 15.4 %** | 0.036 s / 94.7 % |
| -2 s | 0.065 s / 69.4 % | 0.036 s / 94.7 % |
| +5 s | **5.402 s / 3.1 %** | 0.036 s / 94.7 % |
| +15 s | - | 0.036 s / 94.7 % |

The check (`AlignConfig.lrc_offset_check`, `align._lrc_offset`): an alignment without
windows (0.2-0.3 s of Viterbi), the median of (aligned line start - LRC line start),
applied when it is at least 0.3 s and at least half of the lines agree with it within
0.5 s. It never fired on the exact or the jittered line starts.

**Not solved:** an LRC that is off on only part of the song (for example a different
bridge). With the last third of every song's lines 2 s / 5 s late: median 0.058 / 0.065 s
but only 67.6 % / 63.5 % within 0.3 s (worst 32.6 / 59.4 s), worse than text-only
alignment (94.2 %), and no warning. A per-section check (drop windows where the
unconstrained alignment disagrees for several lines in a row, like the transcript
"verify" mode) would be the next step.

## 6. Investigation: where the large errors come from, and what was tried

The median is far below 0.1 s, so the work went into the tails. Per song (default model):

| Song | Text: mean / worst | Lines: mean / worst | Hard: mean / worst | Dataset flags |
|---|---|---|---|---|
| Ridgway - Fire Inside | 1.241 / 54.4 | 0.395 / 4.95 | 0.274 / 4.49 | non-lexical singing |
| Pure Mids - The Leader | 1.030 / 12.9 | 0.229 / 3.47 | 0.290 / 3.46 | non-lexical singing |
| Avercage - Embers | 0.618 / 30.2 | 0.299 / 7.61 | 0.274 / 2.25 | |
| Color Out - Falling Star | 0.348 / 22.9 | 0.237 / 6.00 | 0.164 / 3.89 | |
| HILA - Give Me the Same | 0.322 / 6.31 | 0.244 / 3.49 | 0.131 / 1.82 | |
| Tom Orlando - The One (feat. Tina G) | 0.309 / 25.5 | 0.263 / 25.5 | 0.299 / 25.5 | duet |
| the other 14 songs | 0.028-0.167 | same or better | | |

Causes found by looking at the frames (`lp` argmax and vocal level around each error):

1. **The CTC model hears nothing.** In distorted or heavily processed rock vocals
   (Avercage, Color Out) the model emits blank for whole phrases. The Viterbi then has
   only weak evidence and can squeeze a word into the tail of the previous held note
   (Color Out: "i've" placed at 133.3 s right after "known", true 156.2 s, across a 22 s
   break). Line windows stop this (worst 22.9 s -> 6.0 s).
2. **Lines sung more often than written.** The last words of the last line of "The One"
   drift 25 s to the outro, where the singers repeat the phrase; the lyrics list it
   once. No window can know which repeat is meant when the LRC has no time for the
   repeat either; the user fixes it in the sync editor (manual tools work everywhere).
3. **Non-lexical singing** (scat, "oh-oh") is not in the lyrics but is in the vocal stem;
   text-only alignment absorbs it with the wrong words. Line windows fix most of it
   (Ridgway 1.24 -> 0.40 s mean).
4. **Duets.** The featured singer is partly separated into backing vocals, so the lead
   stem lacks some lines.

Tried, measured on the dev songs (wav2vec2 unless HuBERT is named), **not adopted**
(code removed again unless noted):

| Idea | Result |
|---|---|
| Per-chunk input normalisation instead of song-level | no gain (HILA mean 0.351 vs 0.332 s; Quentin identical) |
| Blank bias -1 / -2 / -3 nats | no change (mean 0.186 -> 0.186 / 0.186 / 0.186 s) |
| Penalty for unvoiced pauses inside a word (0.2-1 nats/frame) | worse: mean 0.263 -> 0.304 s (first 6 songs); a song-final word jumped 52 s to the outro |
| Rejoin words torn across a silence (post-processing) | worse: mean 0.186 -> 0.212 s (8 songs), the same 52 s failure |
| Silence threshold -30/-35/-40 dB, penalty 2-4 nats | ±0.002 s |
| Move word starts back over voiced pre-onset frames (0.1/0.3 s) | median 0.043 -> 0.042 s (HuBERT), mean unchanged |
| Constant onset shift (adopted) | median 0.043 -> 0.035 s, mean 0.137 -> 0.129 s (HuBERT); bias now -0.004..+0.004 s on dev and test |
| Cost per frame of a pause inside a line, 0.01/0.03/0.1 nats (kept as `line_gap_cost`, default 0) | dev text 0.137 -> 0.116 / 0.126 / 0.141 s but test text 0.279 -> 0.310 / 0.307 / 0.428 s; with lines, test worst 25.5 -> 4.95 s at 0.03 but dev slightly worse: not consistent |
| Whisper word windows as hard constraints ("windows") | worse: mean 0.129 -> 0.450 s (8 songs, turbo with VAD). Whisper's word times on singing: median error 0.09-0.15 s but p90 0.4-1.1 s, and a wrong ±1 s window forces the wrong place |
| Whisper windows only where the CTC result disagrees by > 2 s for ≥ 3 words in a row ("verify", kept as the opt-in mode) | neutral to slightly worse: mean 0.129 -> 0.137 s |
| Both, re-measured by the verification on all 20 songs (wav2vec2, turbo without VAD) | worse than text only (mean 0.209 s, 94.2 % within 0.3 s): "windows" 0.576 s / 91.1 %, "verify" 0.652 s / 90.1 %; medians 0.037 / 0.038 s |

## 7. Transcription (lyrics source of last resort)

`transcribe.Transcriber` on the separated lead vocal, fp16, beam 5, word timestamps,
`condition_on_previous_text=False`, `hallucination_silence_threshold=2 s`. Word error rate
against the dataset lyrics (lower-cased, punctuation removed, word-level Levenshtein),
pooled over words:

| Model | VAD | Songs | WER | Time per song |
|---|---|---|---|---|
| **large-v3-turbo (default)** | **off (default)** | 20 | **26.6 %** | 19.7 s (6-42 s) |
| large-v3-turbo | off | first 5 | 29.2 % | 22.1 s |
| large-v3 | off | first 5 | 29.9 % | 34.3 s |
| large-v3-turbo | Silero VAD on | 14 | 30.8 % | (GPU heavily shared, not comparable) |
| large-v3-turbo | off | same 14 | 28.0 % | |

* **large-v3 is not better than turbo here** (29.9 % vs 29.2 % on the same five songs)
  and 55 % slower, so turbo (1.6 GB instead of 3.1 GB) is the default.
* **Silero VAD hurts on separated vocals**: it dropped whole sung passages. Worst cases
  with VAD on vs off: Avercage 75.1 % -> 58.2 %, Pure Mids 75.4 % -> 28.9 %, Moon I Mean
  60.3 % -> 27.6 % (one song got slightly worse: Quentin 9.1 % -> 11.4 %). VAD is off by
  default.
* Per song (turbo, VAD off) the WER ranges from 4.9 % to 58.2 %; the worst are the
  distorted rock vocal (Avercage), fast rap (Wordsmith 51.5 %) and dense pop
  (Explosive Ear Candy 46.0 %). That is why transcribed lyrics are labelled unchecked:
  roughly one word in four needs correcting.
* Times are with the GPU shared; the 5070 Ti alone will be faster.
* Transcribed lines: words are grouped into lines at Whisper segment starts, pauses of
  1.2 s, sentence ends and capitalised words (only once a line has 3 words), a pause of
  3 s always ends a line, and lines over 10 words are split at their largest pause
  (`transcribe.words_to_lines`, tested in `test_align_lines.py`).

## 8. How to reproduce

```
source scripts/devenv.sh
D=$JK_BUILD/eval/jamendolyrics; M=$JK_BUILD/align-venv/models; PY=$JK_BUILD/align-venv/Scripts/python.exe
git clone --depth 1 https://github.com/f90/jamendolyrics "$D"
$JK_BUILD/studio-venv/Scripts/python.exe -I -B studio/bench/alignment_bench.py separate --data $D
for m in facebook/wav2vec2-large-960h-lv60-self facebook/hubert-xlarge-ls960-ft; do
  $PY -I studio/bench/alignment_bench.py emissions --data $D --models $M --model $m --sources lead,mix
  $PY -I studio/bench/alignment_bench.py align --data $D --model $m --songs all \
      --variants mix_text,lead_text,lead_text_nohold,lead_text_holdall,lead_lines,lead_lines_noisy,lead_hard
done
$PY -I studio/bench/alignment_bench.py transcribe --data $D --models $M --whisper large-v3-turbo
$PY -I studio/bench/alignment_bench.py transcribe --data $D --models $M --whisper large-v3 --limit 5
$PY -I studio/bench/alignment_bench.py wer --data $D --whisper large-v3-turbo,large-v3
$PY -I studio/bench/alignment_bench.py report --data $D --results $D/_work/results.wav2vec2-large-960h-lv60-self.global.json --per-song lead_text
```

Model files must be in `$M/hf` first (the Studio never downloads on its own): e.g.
`align.Aligner(model, models_dir=M, allow_download=True)` and
`transcribe.Transcriber(name, models_dir=M, allow_download=True)` fetch them from
huggingface.co at the revisions pinned in `align.MODEL_INFO` and
`transcribe.WHISPER_MODELS`. The separation step needs the
separation workstream's environment (`studio-venv`: demucs, einops, rotary-embedding-torch).

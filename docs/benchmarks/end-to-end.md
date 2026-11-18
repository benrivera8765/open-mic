# End to end through the real app (WORKING ORDER section 9)

Status: **measured on this PC, 17 November 2026, 20:48-21:40 (local time).** The release
`Jukeoke.exe` on a portable test stick drove the real Studio runtime. Three Creative
Commons songs and one corrupt file went through the Studio queue. Each song file was then
verified, rendered at three keys, compared with ground truth, and played on the Stage. The
same stick was also started with the Studio unable to run, in four different ways. Raw
results are in `$JK_BUILD/e2e/` (outside the repository). Nothing from the songs (audio or
lyrics) is in the repository.

Covers section 9 rows "End to end" (Windows half), "Weak computers" and "Lyric alignment"
on real songs, the speed target of section 4.1, and a portable-mode audit of the whole run.

## Summary

- **The queue works.** Three songs went in as a 48 kHz FLAC, a float WAV and an MP4 video,
  with a corrupt file second in the queue. Every song became a `.jksong` that passes
  `jukeoke-cli verify --strict` with 0 errors and 0 warnings. The corrupt file failed in
  0.04 s with a plain message, and the next job started at once.
- **Speed: about 3 minutes for a 4-minute song** (0.68-0.73x the song's length; separation
  is 85-92 % of that):

  | Song | Time |
  |---|---|
  | Feel (Stripped), 4:00.6, lyrics given | 174.6 s |
  | The Homeless Man, 4:05.7, lyrics transcribed | 175.1 s |
  | Crowdpleaser, 3:12.8 | 131.0 s, and 153.3 s in a repeat run |

  The repeat produced bit-identical stems and lyrics.
- **Lyrics timing on real songs.** Word-onset errors against the JamendoLyrics annotation:

  | Song | Median | p90 | Worst | Within 0.3 s |
  |---|---|---|---|---|
  | Feel (Stripped) | 0.045 s | 0.110 s | 0.200 s | 100 % |
  | Crowdpleaser (3.4 words/s) | 0.033 s | 0.073 s | 0.201 s | 100 % |

  Both medians are well under the 0.1 s investigation threshold, and both songs match the
  alignment benchmark's per-song numbers.
- **Key change on real songs.** Renders at +2 and -5 keep the exact length. A frame-by-frame
  chroma comparison finds the expected shift in all 6 renders. At key 0 the render equals
  the plain sum of the karaoke stems: exactly for two songs, within 1.2e-7 for the third.
- **The ghost singer depends on the song.** On the ccMixter song (true stems known) the lead
  residue in the default karaoke mix is **-6.0 dB** with backing vocals on, and -20.8 dB
  with backing vocals off. The separation benchmark measured -6.4 / -21.0 dB on a 45 s
  excerpt of the same song; its mean over 18 songs is -15.9 / -24.3 dB. On this song the
  lead/backing model leaves much of the lead in the backing stem. This goes on the listening
  checklist.
- **Ticker in the real app (1440x900).**
  - Letter under the marker: 0 wrong letters across all three whole songs.
  - Lookahead: on the fast rap song it drops to **1.46 s** (1.7 % of sung time below 2 s),
    with the font already at its 0.6x floor. WORKING ORDER 4.4 asks for at least 2-3 s, so
    this is not met for dense songs (the open item of D-96).
- **Weak computers.** The app opens in 0.6 s, plays, and shows the Studio as unavailable with
  a plain reason in all four cases:
  - GPU hidden: reason after 0.9 s.
  - Windows/Studio folder renamed: reason after 0.6 s.
  - A python.exe that never answers: reason after 64.6 s, then the process is killed.
  - A python.exe that crashes: reason after 0.6 s.

  No hang, no crash, and no leftover process.
- **Portable audit.** The app and the Studio wrote nothing to `%USERPROFILE%\.cache`,
  `%APPDATA%`, `%LOCALAPPDATA%` or `%TEMP%`. Exceptions and caveats:
  - Explorer's two MuiCache values for `Jukeoke.exe` (D-41).
  - Two shader-cache folders written by Direct3D and the NVIDIA driver. They cannot be
    attributed: other GPU programs were open too (section 7).
  - Every Studio cache landed on the stick. The CUDA JIT cache is 242 MB after the runs.
- **Three integration bugs found and fixed** (section 8):
  - The Studio screen stayed on "Still checking..." for good on a capable PC.
  - A corrupt file was reported as "no sound track".
  - Every `permissive` song failed `verify --strict`.

## 1. Set-up

| | |
|---|---|
| PC | Ryzen 7 9700X, 32 GB, RTX 5070 Ti 16 GB (driver 621.24), Windows 11 26200, internal NVMe SSD |
| App | release `Jukeoke.exe` built with `app/scripts/build-windows.sh` (GNU toolchain): 12,432,398 bytes, 21 imported DLLs, all part of Windows; UI tests 121 passed in that build |
| Studio | relocatable runtime built with `scripts/build-studio-runtime.sh` from the final `studio/` (CPython 3.12.16, torch 2.14.1+cu130), 4.281 GB / 18,769 files; `check-studio-runtime.sh --static-only` 10 passed, 0 failed |
| Models | default set copied from `$JK_BUILD/models-cache`: `full` preset separation weights (3 files) + wav2vec2-large-960h-lv60-self + faster-whisper large-v3-turbo, 3.820 GB |
| Stick | `$JK_BUILD/e2e-stick`, laid out as WORKING ORDER 4.8 (`Windows/Jukeoke.exe`, `Windows/resources/`, `Windows/Studio/`, `Library/`, `Recordings/`, `Models/`, `Settings/`); the app found it as portable ("the app is in a 'Windows' folder next to a 'Library' folder") |
| Output | the engine's silent `null:48000` device, master volume 0 (`Settings/settings.json` written before each start): nothing was played aloud |
| Driver | `app/ui/scripts/real-app-e2e.mjs`: the exe started with WebView2's DevTools port, Playwright over CDP, Tauri commands (`studio_inspect`, `studio_submit`, `studio_status`, `library_list`) through the page, `studio://job` events recorded with wall-clock times, UI clicks for the Library, Stage and Studio screens |

Two app builds were used. The first ran the main queue (section 3). After the bugs in
section 8 were fixed, both the app and the Studio runtime were rebuilt, and the second build
ran everything else: queue 2, the Stage screenshots, the weak-computer runs, and the
re-verification of the song files. Section 3's song files were made by the first build. The
fixes do not touch the pipeline's audio or lyrics path, and queue 2 reproduced a song
bit-identically on the second build.

## 2. Songs and other inputs

Prepared by `studio/bench/e2e_inputs.py` into `$JK_BUILD/e2e/inputs` (never committed):

| # | Song (artist - title) | Source and licence | Prepared as | Lyrics given |
|---|---|---|---|---|
| 1 | Cortez - Feel (Stripped), 4:00.6, solo singer with guitar | JamendoLyrics (Jamendo track 1481500), **CC BY** | **48 kHz 24-bit FLAC** with tags. The dataset MP3 was decoded with the Studio's decoder, converted once with soxr VHQ, and scaled by 0.866 because the decoded MP3 peaks at 1.154 | the dataset's lyrics text (`user_text`) |
| 2 | (corrupt file) | 2,000,000 seeded random bytes named `corrupt.m4a` | FFmpeg's probe reads it as an "LRC lyrics" subtitle file with no audio | - |
| 3 | coruscate - The Homeless Man, 4:05.7, lead plus backing vocals at almost the same level (RMS -18.2 vs -19.1 dBFS) | ccMixter upload 68853 (https://ccmixter.org/files/Coruscate/68853), **CC BY 3.0** | 32-bit float WAV = the upload's own lead + backing + instrument stems, so the true stems are known | none: `auto`, so the lead vocal was transcribed |
| 4 | Jason Miller - Crowdpleaser, 3:12.8, hip-hop, 521 words (3.4 words/s) | JamendoLyrics (Jamendo track 1144143), **CC BY-NC-SA** (used only for this non-commercial evaluation) | **MP4 video**: a static 320x180 H.264 picture at 1 frame/s plus AAC 256 kbit/s, made with PyAV. Decoded, it has the same number of samples as the MP3 and a lag of 0 samples (cross-correlation), so the AAC priming is removed by the edit list | the dataset's lyrics text |

Ground truth: JamendoLyrics word start times (`annotations/words/*.csv`, MIT) for songs 1
and 4, and the ccMixter stems for song 3. ccMixter titles and licences for the whole
evaluation set are in `$JK_BUILD/eval/ccmixter_voices/LICENSE-EVAL.md`; JamendoLyrics
licences are in its `JamendoLyrics.csv`.

## 3. The queue through the real app

Queue 1 (first build, 21:08:29-21:16:40):

1. The exe started and the window was ready after 1.0 s.
2. The background probe reported the Studio available after **9.5 s**: a cold first start
   with an empty CUDA cache.
3. `studio_inspect`, then `studio_submit`, for all four files back to back. The two
   Jamendo songs got their lyrics as text; the ccMixter song got lyrics mode `auto`.
4. The driver waited for every `studio://job` to end.

| Input | `studio_inspect` | Job | Time in the app |
|---|---|---|---|
| solo-48k.flac | flac, 48000 Hz, 2 ch, 24 bit, lossless, 240.56 s, title/artist from tags (0.10 s) | **done** | 174.6 s |
| corrupt.m4a | error `decode_failed` "This file has no sound track." (0.04 s) | **failed** in 0.04 s, same message | - |
| backing-mix.wav | wav, pcm_f32le, 44100 Hz, 2 ch, 32 bit, 245.71 s, no tags (title from file name; the job's metadata set it) | **done** | 175.1 s |
| wordy.mp4 | mp4, aac, 44100 Hz, `has_video: true`, lossy, title/artist from tags | **done** | 131.0 s |

The job after the corrupt file started in the same millisecond as the failure (02:11:34.121 UTC). The app's wall times (from
`job.started` to `job.done`) agree with the Studio's own `total_seconds` within 0.03 s.
The three songs took 480.7 s of processing; the whole queue took 490.9 s from launch.

Queue 2 (second build, 21:22:55, with the fixes of section 8): `corrupt.m4a` then
`wordy.mp4` again.

- The probe reported the Studio available after 3.2 s (warm caches).
- The corrupt file failed with the new message: "This file could not be read as audio or
  video. It may be damaged, or it is not a music or video file."
- Crowdpleaser took 153.3 s and was saved as `Jason Miller - Crowdpleaser (2).jksong`, so
  nothing was overwritten. Compared with run 1: all four stems are identical integer for
  integer, the lyrics JSON is identical, and the manifest is identical except
  `created_utc`. The copy was then deleted, so the Library holds the three songs.

Studio screen during and after queue 2:
`$JK_BUILD/e2e/run-queue2/studio-queue-{running,finished}-1440x900.png`. The running
screenshot shows "Separate 44 %" and "Separating the lead vocal from the backing vocals".
The finished screenshot shows corrupt.m4a as Failed, with the message in a red-edged box,
and wordy.mp4 as Done with "Sing it now". Queue 1's screenshots show the bug fixed in
section 8.1 instead.

## 4. Speed (WORKING ORDER 4.1: "a few minutes" for four minutes)

Per stage, in seconds, from the Studio's `job.done` report:

| Song | Length | Source | decode | separate (model load) | lyrics | align | analyse | write | **total** | total / length |
|---|---|---|---|---|---|---|---|---|---|---|
| Feel (Stripped) | 4:00.6 | 48 kHz FLAC, text | 0.33 | 155.54 (1.93) | - | 4.28 | 3.16 | 11.33 | **174.64** | 0.73 |
| The Homeless Man | 4:05.7 | float WAV, transcribed | 0.13 | 148.86 (1.59) | 12.25 | 1.95 | 0.56 | 11.33 | **175.08** | 0.71 |
| Crowdpleaser | 3:12.8 | MP4/AAC, text | 0.16 | 120.09 (1.33) | - | 1.67 | 0.43 | 8.60 | **130.96** | 0.68 |
| Crowdpleaser, repeat (queue 2) | 3:12.8 | MP4/AAC, text | 0.16 | 136.34 (1.27) | - | 6.20 | 2.01 | 8.61 | **153.33** | 0.80 |

- **A 4-minute song takes about 3 minutes** on the RTX 5070 Ti, in full quality (fp32), with
  lyrics given or transcribed.
- Separation dominates (85-92 %). Transcription adds 12 s; writing (four 24-bit FLAC stems
  plus the decode-and-sum check) about 11 s.
- The first job in a fresh Studio process pays about 3-5 s more in align and analyse:
  - align 4.3 and 6.2 s for a first job, against 1.7-2.0 s for later jobs (loading the
    1.26 GB alignment model cold);
  - analyse 2.0-3.2 s against 0.4-0.6 s (librosa's numba compilation).
- **Peak VRAM** on the card was 6,766 MiB. That includes 2,465 MiB held by other programs
  before the run, so the Studio used about 4.3 GB. Mean GPU utilisation over queue 1 was
  81 %.
- **Run-to-run variation is real and not explained.** The same song took 131.0 s and then
  153.3 s (separation 120.1 against 136.3 s), with bit-identical output. The repeat came
  right after the runtime had been rebuilt; every stage was slower, align and analyse by
  3-4x, which points at the CPU side. Nothing else of this workstream was running. Windows
  Defender's first scan of the 18,769 freshly written runtime files is a plausible cause
  but was not measured.
- For comparison, D-158 measured 193.3 s for Feel (Stripped) from its 44.1 kHz MP3, run by
  `make` while other workstreams were busy (separation 171.2 s). Here, through the app and
  from 48 kHz, it took 174.6 s (separation 155.5 s). The separation benchmark measured
  152-160 s for a 4:04 song.
- **Independent re-measurement (verifier, 21:47-21:52).** The same `solo-48k.flac` went
  through the stick's packaged Studio runtime over the protocol, started the way the app
  starts it. It was the third job of that process, after a cancelled job and a 30 s
  excerpt. It took **188.9 s**: decode 0.73, separate 171.47 (model load 3.14), align 3.88,
  analyse 0.63, write 12.19. That is 8 % slower than the 174.6 s above, and 0.79x the song's
  length. The output matched queue 1's song exactly: all four stems integer for integer,
  the lyrics JSON, and the manifest apart from provenance `app`, `created_utc` and
  `metadata_source`. The word onsets were the same (median 0.045 s, p90 0.110 s, worst
  0.200 s). Total GPU memory peaked at 10.2 GB against 6.8 GB in queue 1, so another
  program was probably using the card at the same time. It could not be attributed,
  because nvidia-smi shows no per-process memory under WDDM. So read "about 3 minutes" as
  roughly 2.9-3.2 minutes on this PC.
- The probe answered 9.5 s after a cold first start of the app, and 2.5-3.2 s once the
  caches on the stick were warm.

## 5. The song files

`studio/bench/e2e_metrics.py`. `verify` and the renders use `jukeoke-cli` rebuilt from the
final sources.

| Song | File | Rate | `verify --strict` | Timed graphemes | Key / tempo (Studio) | Stems sum vs decoded source |
|---|---|---|---|---|---|---|
| Feel (Stripped) | 111,125,066 B | 48 kHz | PASS, 0 errors, 0 warnings (1.4 s) | 1,215 | D major, 99.4 BPM | 0.0 (24-bit FLAC source: exact) |
| The Homeless Man | 142,271,051 B | 44.1 kHz | PASS, 0 errors, 0 warnings (1.3 s) | 2,441 | C minor, 89.1 BPM | 1.2e-7 |
| Crowdpleaser | 109,728,332 B | 44.1 kHz | PASS, 0 errors, 0 warnings (1.0 s) | 2,139 | C# minor, 103.4 BPM | 6.0e-8 |

- The three files are 110-142 MB, inside WORKING ORDER 4.8's 80-150 MB per song.
- The 48 kHz song's provenance lists exactly one conversion into the models
  (48000 -> 44100) and one back per estimate (44100 -> 48000 for vocals, lead and drums),
  all soxr VHQ. The 44.1 kHz songs list none.

### 5.1 Offline renders at keys 0, +2 and -5

`jukeoke-cli render --format wav32f --semitones K`: the playback graph with the song's
default mix.

| Song | Key | Render time | Frames = song | Peak | Check |
|---|---|---|---|---|---|
| Feel (Stripped) | 0 | 0.6 s | yes | 0.850 | max diff vs stem sum **0.0** |
| | +2 | 25.2 s | yes | 0.884 | chroma shift **2** (score 0.78; unshifted 0.09) |
| | -5 | 29.9 s | yes | 0.837 | chroma shift **7 = -5** (0.63; next best 0 at 0.31) |
| The Homeless Man | 0 | 0.6 s | yes | 1.516 | max diff vs stem sum **1.2e-7** |
| | +2 | 25.4 s | yes | 1.547 | chroma shift **2** (0.73; next best 1 at 0.40) |
| | -5 | 27.7 s | yes | 1.586 | chroma shift **7** (0.39; next best 0 at 0.20) |
| Crowdpleaser | 0 | 0.5 s | yes | 0.899 | max diff vs stem sum **0.0** |
| | +2 | 18.6 s | yes | 1.170 | chroma shift **2** (0.86) |
| | -5 | 22.0 s | yes | 1.185 | chroma shift **7** (0.73) |

- **How the key was checked.** Take 60 s starting at 30 % of the song and compute librosa
  CQT chroma for both renders. Score each circular shift k by the mean, over frames, of the
  correlation between the key-0 frame rolled by k and the same frame of the shifted render.
- A first version compared the mean chroma of the whole window. It found 9 instead of 7 for
  The Homeless Man at -5, so it was too weak a test and was replaced. Pitch accuracy itself
  is proven on tones in `docs/benchmarks/dsp.md` (within 0.0015 cents).
- **Renders above full scale are expected.** The Homeless Man's source is itself a float mix
  that peaks at 1.84. A key change can raise the peak a little: Crowdpleaser goes from
  0.90 at key 0 to 1.17-1.18.
- Rendering at +2 or -5 runs at 8-10x real time on this CPU.

### 5.2 Word onsets against JamendoLyrics

|predicted word start - annotated start|, all words matched in order. The word counts equal
the annotation's: 355/355 and 521/521.

| Song | Path through the app | Median | Mean | p90 | p95 | Worst | Within 0.1 s | Within 0.3 s |
|---|---|---|---|---|---|---|---|---|
| Feel (Stripped) | 48 kHz FLAC, text | **0.045 s** | 0.053 s | 0.110 s | 0.135 s | 0.200 s ("feel", 198.5 s) | 85.1 % | 100 % |
| Crowdpleaser | MP4/AAC, text | **0.033 s** | 0.038 s | 0.073 s | 0.087 s | 0.201 s ("tarnish", 102.6 s) | 96.7 % | 100 % |

The alignment benchmark ran the same songs from the dataset MP3s, text mode, with the lead
vocal separated outside the app (`docs/benchmarks/alignment.md`):

| Song | Median | p90 | Max |
|---|---|---|---|
| Feel (Stripped) | 0.046 s | 0.111 s | 0.592 s |
| Crowdpleaser | 0.033 s | 0.073 s | 0.201 s |

The app, the 48 kHz path and the MP4/AAC container leave the timing where the benchmark had
it. D-158 measured a 0.045 s median on Feel (Stripped) from the MP3 through `make`.

The ccMixter song's lyrics were transcribed (107 lines, 559 words). There is no reference
text, so their accuracy is not measured here. The song file labels them `transcribed`,
unchecked, and the Stage shows "These lyrics were transcribed by the computer and nobody
has checked them yet".

### 5.3 Residual lead vocal (ccMixter song, true stems known)

The whole song (245.7 s), with the separation benchmark's metrics (`quality.py`). Estimate
= the song file's stems; truth = the upload's stems.

| Measure | This run, whole song | Benchmark, 45 s excerpt of this song | Benchmark mean, 18 ccMixter mixes |
|---|---|---|---|
| Lead residue in the default karaoke mix (backing vocals on) | **-6.0 dB** | -6.4 dB | -15.9 dB |
| Lead + backing residue with backing vocals off (drums + other vs instruments) | **-20.8 dB** | -21.0 dB | -24.3 dB |
| Karaoke mix SDR (backing on) | 7.5 dB | 5.7 dB | 18.1 dB |
| SDR with backing off | 13.9 dB | 12.9 dB | 21.1 dB |
| Lead stem SDR / backing stem SDR | 2.5 dB / 1.6 dB | | |
| Backing stem energy vs true backing | -3.0 dB | | |
| Karaoke mix band energy vs truth (<250 Hz / 0.25-4 kHz / 4-8 kHz / 8-16 kHz) | -0.21 / -1.54 / -0.15 / -0.15 dB | | |

- **This song is one of the hardest in the ccMixter set**, and the run matches the
  benchmark's own number for it. The vocals-versus-band split is good: with backing off,
  -20.8 dB. The lead-versus-backing split is not: the karaoke model moves a large part of
  the lead into the backing stem (lead SDR 2.5 dB), so with backing vocals on, a strong
  ghost of the singer remains.
- **The listening checklist** should say: on songs whose backing vocals double the lead,
  try "backing vocals off".
- The mid-band loss of 1.5 dB is the lead's own energy removed from 0.25-4 kHz; it is not
  a dulled instrument. High frequencies stay within 0.15 dB.

## 6. Loading and playing in the real app

For each Library song, `real-app-e2e.mjs stage` did the following at 1440x900:

1. Opened the song from its title strip.
2. Ran the ticker's own letter-under-the-marker check over the whole song on the live
   layout: WebView2 with the skin's lyrics font (sampled every 10 ms of sung time).
3. Sought so that the middle of a word of at least 0.5 s in the middle third of the song
   reaches the marker after 1.3 s of playback.
4. Played, and took a screenshot.

| Song | Sung samples | Wrong letter / backwards / too fast | Font | Lookahead (min) | Under 2 s | Plays (clock events in 1.3 s) | Underruns |
|---|---|---|---|---|---|---|---|
| Feel (Stripped) | 14,618 | 0 / 0 / 0 | 41.1 px (0.73x) | **2.13 s** | 0 | 41, 79.81 -> 81.08 s | 0 |
| The Homeless Man | 14,834 | 0 / 0 / 0 | 33.6 px (0.60x, floor) | **2.00 s** | 1 sample (a letter exactly 2.00 s ahead not fully on screen) | 41, 83.20 -> 84.47 s | 0 |
| Crowdpleaser | 12,066 | 0 / 0 / 0 | 33.6 px (0.60x, floor) | **1.46 s** | 211 samples (1.7 % of sung time) | 41, 71.00 -> 72.27 s | 0 |

Screenshots are in `$JK_BUILD/e2e/run-stage/stage-{1,2,3}-1440x900.png`. Each was looked at:

1. "...hat you want from me | want from me ... what is it that yo": the marker on the "e"
   of "me". The lyrics say "e" of "me" is sung at that instant.
2. "...oing his best but he's alre|ady damned this poor man is living out his car s...":
   the marker at the "e"/"a" of "already", under the transcribed-lyrics notice. The lyrics
   say "y" of "already" at the clock reading taken just after the capture. The capture
   itself takes roughly 0.1-0.2 s, which is what separates the two.
3. "...ir guards go up  score|s up all the applauds go up  and everybody in the": the
   marker on the final "s" of "scores", which is what the lyrics say.

In all three, the key plate reads "<key> (original key)", sung letters are pink, upcoming
letters cream, and the marker mint.

**Finding: lookahead on dense songs.** WORKING ORDER 4.4 wants the coming text readable "at
least two to three seconds" ahead. The ticker shrinks the font to keep 2 s of text on
screen, down to 0.6x (D-96). Crowdpleaser, at 3.4 words/s, hits that floor: for 1.7 % of
the sung time the next letter to be sung within 2 s is not yet fully on screen, at worst
1.46 s ahead. Letting the font go below 0.6x (33.6 px at 1440x900) would fix the number
but cost readability. This is a design decision for the owner and is left as it is.

## 7. Weak computers (WORKING ORDER 4.9)

The same stick, the second app build, the null device, master 0.
`real-app-e2e.mjs unavailable` measured:

- when the window was ready;
- when `capabilities.studio.checking` turned false (polled every 250 ms through
  `studio_status`, which kept answering throughout);
- what the Studio screen shows (screenshot);
- that `studio_submit` is refused at once;
- that a Library song plays: 2 s from 30 s, counting clock events from the engine.

| Case | How | Window ready | Probe answered | Reason shown (verbatim) | Submit | Plays |
|---|---|---|---|---|---|---|
| (a) GPU hidden | `CUDA_VISIBLE_DEVICES=-1` in the app's environment, passed through to the Studio by the supervisor | 0.58 s | **0.87 s** (the Studio's driver fast path: `cuDeviceGetCount` = 0; PyTorch never imported) | "No NVIDIA graphics card with CUDA was found. Songs are created on a stronger computer and can be played anywhere." | refused in 4 ms, same reason | 62 events, 30.00 -> 31.96 s |
| (b) Studio removed | `Windows/Studio` renamed to `Studio.renamed` | 0.57 s | **0.59 s** (nothing started) | "The Studio engine is not on this drive (the Windows/Studio folder is missing), so songs cannot be created here. Songs are created on a stronger computer and can be played anywhere." | refused in 4 ms | 62 events, 30.00 -> 31.96 s |
| (c1) Studio hangs | `Windows/Studio/python/python.exe` = a program that never answers (`hang.c`, `Sleep(INFINITE)`) | 0.60 s | **64.6 s** (60 s start timeout, a 2 s shutdown request, 2 s grace, then killed: "did not exit in time; killing it") | "The Studio engine did not answer within 60 seconds after starting. Details are in Settings/logs/studio.log." | refused in 4 ms | 62 events, 30.00 -> 31.97 s |
| (c2) Studio crashes | `python.exe` = a program that prints a non-protocol line and exits with code 3 (`crash.c`) | 0.59 s | **0.61 s** | "The Studio engine stopped while starting. Details are in Settings/logs/studio.log." (its stderr is in that log) | refused in 4 ms | 62 events, 30.00 -> 31.96 s |

- **What the screens show.** Screenshots:
  `$JK_BUILD/e2e/run-weak/studio-unavailable-{gpu-hidden,studio-missing,python-hangs,python-crashes}-1440x900.png`.
  Each was looked at. Each shows the Studio panel with "Songs are created on a stronger
  computer and can be played anywhere." in bold, then the explanation that playing, key
  change, timing fixes and recording work here, then the reason as a bullet. The rest of
  the jukebox (Library, Sing, Recorder plates, transport, key dial, faders) is normal.
- **Cosmetic:** in (a) and (b) the sentence "Songs are created on a stronger computer..."
  appears twice, in the intro and at the end of the reason. Not changed.
- **No leftover processes.** After each case no Studio process remained (process list
  checked). In (a) the supervisor stopped the Studio right after the probe said no.
- **Restored.** After `Studio.renamed` was moved back, the Studio was available again 2.5 s
  after launch.
- **Not covered:** a real weak PC (an old card, 8 GB of RAM, no NVIDIA driver at all) and a
  slow USB stick. During the 64.6 s wait of (c1), playback was not tried while "checking";
  the window and commands were responsive throughout, and playback worked right after.

## 8. Integration bugs found and fixed in this run

1. **Studio screen stuck on "Still checking..." on the Studio PC.**
   - *Cause.* The UI took `capabilities.studio` from `app_info()` once at start-up, before
     the background probe answers (9.5 s here), and never updated it. The
     `studio://status` event only refreshed the queue. So on a capable PC the Studio screen
     said "Still checking whether this computer can run the Studio..." while songs were
     being made (queue 1 screenshots). The recorder's "make a karaoke song" and the sync
     editor's realign stayed disabled too. The browser mock is never "checking", so the UI
     tests could not see it.
   - *Fix* (`app/ui/src/state/app.ts`, `applyStudioStatus`). Every status from the core,
     event or query, now updates availability, reasons and `checking` in
     `info.capabilities.studio`, and fetches `app_info()` again for the GPU name once the
     probe has answered. `checking` was added to the UI types as an optional field.
   - *Test.* The new test in `app/ui/src/ui.test.tsx` fails on the old code ("timed out
     waiting for the UI") and passes now. UI suite: 121 passed (`npx vitest run`);
     `tsc --noEmit` is clean.
   - *Real app.* Queue 2 shows the queue (section 3).
2. **A corrupt file was reported as "This file has no sound track."**
   - *Cause.* FFmpeg's probe accepts almost any bytes as some format; the random bytes were
     read as LRC subtitles.
   - *Fix* (`studio/jukeoke_studio/media.py`). A file without an audio stream now gets
     "This file could not be read as audio or video. It may be damaged, or it is not a
     music or video file." The detail names FFmpeg's guess. A real video without sound
     still says "This video has no sound track."
   - *Test.* `studio/tests/test_media.py` gained the random-bytes case: 21 passed.
   - *Real app.* Seen in queue 2.
3. **`permissive` songs failed `jukeoke-cli verify --strict`.** This was reported as open by
   the pipeline stream.
   - *Fix.* `crates/jk-format/src/verify.rs` accepts `full`, `clean` and `permissive`; other
     strings remain a warning. `docs/SONG_FORMAT.md` 4.1, `docs/APP_API.md`
     (`studio_submit`), the manifest doc comment, and the UI's preset type and preset list
     ("Permissive licences only", with the missing-backing-vocals warning) now name it.
   - *Tests.* New test `verify_knows_every_studio_preset` in
     `crates/jk-format/tests/format.rs`; `cargo test -p jk-format` passed. The Studio test
     helper `verify_strict` no longer tolerates the warning.
   - *Real model.* `pytest -m gpu -k permissive` makes a real `permissive` song (Kim
     Mel-RoFormer + SYH drums, transcribed lyrics) and runs it through the strict check
     with the rebuilt `jukeoke-cli`: 1 passed (76.9 s).

After all three fixes, the Studio CPU suite gave 208 passed and 1 skipped (the live
LRCLIB test), 24 deselected, in 80.1 s
(`python -I -m pytest -q -x -p no:cacheprovider` in `studio/`, `JUKEOKE_CLI` set to the
rebuilt CLI).

## 9. Portable-mode audit of the whole run

`scripts/user-dirs-audit.py`:

- **Snapshots.** Size and modification time of every file and folder under
  `%USERPROFILE%\.cache`, `%APPDATA%` and `%LOCALAPPDATA%` (which holds `%TEMP%`): 1.17
  million entries. Taken before the first app start (file walk finished 21:04:09) and after
  the last (21:30:56).
- **Registry.** Every HKCU key, value or data that mentions "jukeoke" or "e2e-stick".
- **The window.** Ten app starts, two Studio job queues, the Stage run, the four
  weak-computer runs, and the rebuild of the app and the Studio runtime.

| Group | Entries | What they are | Written by |
|---|---|---|---|
| Names the app or the Studio would use | 42 | all in `%LOCALAPPDATA%\uv\cache` (archive, sdist and interpreter entries for `jukeoke_studio` 0.1.0) | **uv, during `scripts/build-studio-runtime.sh`** on the build PC: the build tool, not the app. The app and the Studio never run uv |
| Windows' own | 10 | `D3DSCache` (1 file changed 21:10:30), Credentials, ActionCenterCache, PowerShell (this session's commands), WebCache, Recent, Remote Desktop client trace | Windows. **Finding:** the D3DSCache write falls inside queue 1 (see below) |
| Measuring tools | 81 | Claude Code's own data and temp folders, Node's compile cache | the tools that ran the test |
| Everything else | 179 | Docker logs and disk, Firefox, OneDrive, Edge, Discord, Spotify, Codex, Windows sign-in caches; `npm-cache\_logs` (npm during the app build and `npx`); `Temp\uv-setuptools-*.lock` (uv, build); `Temp\notificationimages`; Edge's updater (BITS); `NVIDIA\DXCache` (2 new files, 21:10:30 and 21:24:56) | other programs and the build tools. **Finding:** the DXCache writes fall inside queues 1 and 2 |

- **Nothing written by the app or the Studio was found:**
  - nothing under `%USERPROFILE%\.cache`;
  - nothing named Jukeoke, EBWebView/WebView2, Python, pip, torch, Hugging Face, NVIDIA
    ComputeCache, numba, matplotlib, CTranslate2 or librosa in any audited folder;
  - no new file in `%TEMP%` that comes from the app.
- **Registry: two new values**, both Explorer's MuiCache for
  `...\e2e-stick\Windows\Jukeoke.exe` (`ApplicationCompany`, `FriendlyAppName`), which
  Windows records for every program that shows a window (D-41). There is no audio
  PolicyConfig entry (D-88), because this run used the null output device and never opened
  WASAPI.
- **Finding: shader caches.** Direct3D's `D3DSCache` and the NVIDIA driver's `DXCache` got
  new or changed files at 21:10:30 (queue 1) and 21:24:56 (queue 2). The app's WebView2 GPU
  process compiles shaders through Direct3D, so it may have caused them. Firefox, Discord
  and other GPU programs were open too, so they cannot be attributed without a process
  monitor (none is installed). They belong to the class of D-41: written by the OS or the
  driver for any program that renders with the GPU, and naming none of our data. Reported,
  not solved.
- **What the run wrote on the stick:**
  - `Settings/cache/cuda`, the CUDA JIT cache: 127 MB after queue 1, 242 MB after queue 2
    and the second runtime build, 59-69 files.
  - `Settings/cache/numba`: 0.2 MB.
  - `Settings/webview-windows`: 72.8 MB.
  - `Settings/tmp/studio`: empty after every job.
  - `Models/manifest.json`: the first-use hash records.
  - `Settings/logs`: `app.log`, `studio.log`.
- **Tool output kept off the audited folders.** The tools' own scratch files went to
  `$JK_BUILD/tmp` (`TEMP`, `TMP`, `NUMBA_CACHE_DIR` set for the metric scripts and
  pytest). One stray copy made by this session in `%TEMP%` (`app.ts.fixed`) was found by
  the audit and deleted.

## 10. Size of the test stick

| Folder | Bytes | Files |
|---|---|---|
| `Windows/Jukeoke.exe` | 12,432,398 | 1 |
| `Windows/resources` (demo song) | 2,425,490 | 1 |
| `Windows/Studio` | 4,280,623,964 | 18,769 |
| `Models` (default set + manifest.json) | 3,819,956,673 | 22 |
| `Library` (3 songs) | 363,124,449 | 3 |
| `Settings` (CUDA cache 242 MB, WebView2 profile 73 MB, logs) | 315,055,969 | 427 |
| **Total** | **8,793,618,943** | 19,223 |

The Windows app, the Studio and the models come to 8.115 GB raw, without the Mac app, the
songs or the caches. Exact exFAT figures and the budget discussion are in
`docs/STUDIO_RUNTIME.md`.

## 11. Not covered here

- **macOS.** Section 9 also wants the song files to load and render on the macOS runner.
  There is no GitHub access on this PC (D-09), so the renders above are Windows only.
- **`../test-songs/` is empty.** The owner added no songs, so CC material stood in.
- **Hardware.**
  - A physical exFAT USB stick and a different drive letter. The runtime stream ran the
    Studio from a `subst` drive in `docs/STUDIO_RUNTIME.md`; this run used a folder on the
    internal SSD.
  - Real audio output (the null device only).
  - A real weak PC.
- **App features not exercised:** the sync editor's realign through the real app, the
  online lyrics lookup (network), and model downloads (the models were copied in).
- **Transcription accuracy** on the ccMixter song: no reference lyrics.
- **The cause of the 131 s against 153 s variation** on the same song.

## 12. Reproduce

```
source scripts/devenv.sh
bash scripts/build-studio-runtime.sh "$JK_BUILD/e2e-stick"
CARGO_TARGET_DIR="$JK_BUILD/target-e2e" bash app/scripts/build-windows.sh "$JK_BUILD/e2e/app-dist/Windows"
#   copy Jukeoke.exe + resources/ into e2e-stick/Windows; the default models into e2e-stick/Models
$JK_BUILD/studio-venv/Scripts/python.exe -I studio/bench/e2e_inputs.py --eval $JK_BUILD/eval --out $JK_BUILD/e2e/inputs
$JK_BUILD/studio-venv/Scripts/python.exe -I scripts/user-dirs-audit.py snapshot $JK_BUILD/e2e/audit/before.json --stick $JK_BUILD/e2e-stick
cd app/ui
node scripts/real-app-e2e.mjs queue <stick>\Windows\Jukeoke.exe <stick> <out> jobs.json   # [{input,title,artist,lyrics_text_file,expect?}]
node scripts/real-app-e2e.mjs stage <exe> <stick> <out>
CUDA_VISIBLE_DEVICES=-1 node scripts/real-app-e2e.mjs unavailable <exe> <stick> <out> gpu-hidden
#   (b) rename Windows/Studio; (c) put a hanging / crashing python.exe in Windows/Studio/python
cd ../../studio
$JK_BUILD/studio-venv/Scripts/python.exe -I bench/e2e_metrics.py --library <stick>/Library \
    --cli $JK_BUILD/target-e2e/release/jukeoke-cli.exe --eval $JK_BUILD/eval --inputs $JK_BUILD/e2e/inputs --out $JK_BUILD/e2e/metrics
$JK_BUILD/studio-venv/Scripts/python.exe -I scripts/user-dirs-audit.py snapshot after.json --stick <stick>
$JK_BUILD/studio-venv/Scripts/python.exe -I scripts/user-dirs-audit.py diff before.json after.json --out diff.json
```

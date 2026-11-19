# Practice mode: the melody guide (WO2 Stage 0)

Work Order 2 sections 4.1, 6 ("Guide builder") and 9 ("Guide accuracy", "Guide on real
songs"). Measured 2026-11-19 on the build PC (Ryzen 7 9700X, RTX 5070 Ti, Windows 11).
Stage 7 merges this file into `docs/benchmarks/practice.md`.

**Decision: the plain Rust builder (`jk-pitch::guide`, pYIN + note segmentation) is used
alone, everywhere.** At the segmentation's default tuning its note F is equal to or
better than the best neural tracker fed through the same note segmentation in every set;
with the segmentation retuned for the neural curves, SwiftF0 and FCPE gain up to 0.03 F
on separated vocals and tie on real singing (section 6.1), which is still "close" under
WO2's rule. It needs no model (0 bytes added to the stick), it builds a guide for a
4-minute vocal in 1.17 s on one CPU core, and it runs the same on the Mac, so the Mac can
add a guide to an old song itself. Numbers in section 6.

Evidence lives in `$JK_BUILD/practice-guide/` (outside the repository):
`results.json`, `speed.json`, `timing-neural.json`, `guides/<method>/*.json`,
`curves/<method>/*.csv`, `pictures/*.png`, the synthetic singer's WAVs and ground truth,
the mixes and their separated lead vocals.

## 1. What was built

| Piece | Where |
|---|---|
| Guide builder (Rust, no threads, no I/O) | `crates/jk-pitch/src/guide.rs`, `src/guide/{resample,pyin,segment,octave}.rs` |
| Command-line front end (WAV in, JSON out; `--curve` runs only the note segmentation on another tracker's curve; `--set name=value` for sweeps) | `crates/jk-pitch/examples/guide_wav.rs` |
| Synthetic singer, neural tracker wrappers, metrics, pictures | `studio/bench/practice/{synth,trackers,metrics,plots}.py` |
| Benchmark driver | `studio/bench/practice_guide_bench.py` |

No dependencies were added: the builder uses only the crate's shared `realfft` and
`serde` (the "guide" section of `crates/jk-pitch/Cargo.toml` stays empty).

### 1.1 The builder

1. **Analysis signal.** Channels averaged (a lead stem, not a microphone), decimated by
   an integer factor to about 16 kHz (48 kHz: /3, 44.1 kHz: /3 to 14.7 kHz, 96 kHz: /6)
   with a Blackman-windowed sinc. Every rate lands on the same 10 ms frame grid in seconds.
2. **pYIN pitch curve** (Mauch and Dixon 2014; parameter choices as librosa's `pyin`,
   written from the paper): YIN difference function per frame by one FFT
   cross-correlation (32 ms window, lags for 60-1100 Hz); 100 thresholds under a
   Beta(2, 18) prior with a Boltzmann prior over troughs give weighted candidates; an
   HMM over 20-cent pitch bins x {voiced, unvoiced} (switch 0.01, at most 35.92
   octaves/s) is decoded with Viterbi; each voiced frame reports the exact candidate
   (parabolic interpolation on the raw difference function) nearest the decoded bin, so
   the 20-cent bins limit only the path, not the accuracy.
3. **Notes** (shared by every tracker). Read first: UltraSinger (one note per lyric
   word or syllable, the most frequent pitch in it: hangs notes on lyric timings, which
   trap 9 forbids), SOME (a learned segmenter: needs a model) and pYIN's note mode (an
   HMM over semitones: quantises to the grid). This segmentation keeps the pitch as
   measured and cuts only where the audio says so:
   - frames 35 dB under the vocal's loud level (95th percentile) are not singing;
     dropouts of up to 30 ms inside a steady pitch are bridged;
   - voiced stretches are cut at level dips of 6 dB (syllables sung legato);
   - inside a stretch: 3-frame median; vibrato flattened to its centre line (alternating
     extrema 60-120 ms apart, swings 15-250 cents, steady centre, at least 3
     half-cycles); an optimal piecewise-constant fit (least squares + 16000 cents^2 per
     extra note, notes at least 40 ms);
   - glides (segments under 60 ms, or up to 300 ms with fewer than half their frames
     within 30 cents of their median; a short first segment sliding into a note twice
     as long; a short last one) join the note they lead into, or the one they leave at a
     phrase end. A slide's start is moved back to where the pitch leaves the previous
     note by more than 15 cents. The pitch curve keeps the slide's shape; the note
     records `glide_in_s`;
   - neighbours less than 50 cents apart are one note; a note's pitch is the median of
     its steady part, in fractions of a semitone, tuning included;
   - unpitched: a stretch with less than 25 % of its length in notes whose robust
     (Theil-Sen) slope is under 600 cents/s; a phrase (gaps under 0.3 s) at least 60 %
     unpitched, or of at least five short syllables (median under 0.25 s) at least 25 %
     unpitched, becomes unpitched as a whole (rap);
   - confidence = sqrt(mean voicing probability) x (0.4 + 0.6 x share of frames within
     50 cents) x min(1, length / 0.1 s) x 0.6 if quieter than 25 dB under the loud level;
   - tuning = circular mean of the confident notes' offsets from the semitone grid,
     weighted by length, with its strength (resultant length, 0..1);
   - **independent octave check** on each note's average magnitude spectrum (window of 8
     periods, 64-160 ms): partials half-way between the harmonics (octave too high) or
     missing odd harmonics (octave too low), measured in dB above the spectral floor. A
     flagged note's confidence is multiplied by 0.4 (so it is drawn dim and not graded).
4. **Output** (`Guide`): notes (start, end, MIDI with fractions, confidence, pitched or
   unpitched, octave check, glide-in), the 10 ms pitch curve (Hz, voicing probability,
   level), tuning in cents and its strength, method `jk-pitch pyin+notes` and version
   `0.1.0`. Lyrics are never read: words get notes by overlap later (Stage 2).

Unit tests (`cargo test -p jk-pitch --release guide`, 10 tests, all pass): steady tones
E2..C6 within 5 cents and one note each, start/end within 40/50 ms; vibrato +-50 cents
is one note; a legato scale is one note per step; silence and white noise give no
pitched note; A=436 Hz measured within 2 cents; the same pitch within 5 cents at 16,
22.05, 44.1, 48 and 96 kHz; the threshold prior; the fit and the vibrato flattening; the
octave check flags a tone claimed an octave too high as `maybe_high` and one claimed an
octave too low as `maybe_low` (MIDI 45-72).

Sample rate, measured on the whole synthetic set (each voice resampled with soxr VHQ,
then built): note F 0.975 at 32, 44.1, 48 and 96 kHz and 0.971 at 22.05 kHz (no
decimation there); separated 0.872-0.873, and 0.868 at 22.05 kHz; tuning within 0.5 cents.

## 2. Test material

### 2.1 Synthetic singer (`studio/bench/practice/synth.py`)

Source-filter voice, deterministic (seeded): Rosenberg glottal pulses with per-cycle
jitter (0.6 %) and shimmer (5 %), glottal-modulated aspiration, four time-varying formant
resonators per vowel (a e i o u) scaled per voice, F1 lifted above f0 on high notes;
unvoiced consonants (s, sh, f, t, k) as filtered noise before the vowel; voiced
consonants between legato syllables as 10 dB level dips; breaths (300-500 ms) before
every phrase. Tuned to **A = 436 Hz** (-15.86 cents). Four voices across **E2..C6**:
bass E2-C4, tenor C3-A4, alto G3-E5, soprano D4-C6; 61-65 s each, 365 sung notes and
126 spoken syllables in all. Each voice sings three rounds of six phrase types: legato
syllables (30 % scooped 100-200 cents, 30 % joined legato), held notes of 1.5-3 s,
fast runs (6-9 notes of 90-140 ms on one vowel), two or three notes per syllable, leaps
with portamento slides of 120-250 ms, and spoken/rapped lines (8-12 voiced syllables of
120-260 ms with speech-like contours: ground truth "unpitched"). Vibrato 5-6.5 Hz,
+-30-70 cents, delayed 0.2-0.35 s, on every note of 0.45 s or longer. No lyrics exist.

### 2.2 Through the real separation

Each voice mixed (vocal = accompaniment RMS, 0 dB) with a 60 s excerpt (from 20 s) of a
ccMixter instrumental (CC BY): bass ccm-29611, tenor ccm-31416 + that upload's backing
vocals at -6 dB, alto ccm-59840 + backing at -6 dB, soprano ccm-67424; then
`Separator(device="cuda").separate(mix, 48000, preset="full")`, the Studio's own call,
and its `lead_vocal` stem. Separation took 47-49 s per 61-65 s mix (108 s for the
first, with model loading).

The separated lead holds the synthetic singer at residual -19.4 / -18.9 / -15.9 dB (bass /
tenor / soprano; least-squares gain 0.98 / 0.98 / 0.75). **Alto is a separation failure:**
between about 26 and 48 s the karaoke model kept the other upload's backing singer as
the lead and put the synthetic lead into "backing" (gain 0.28, residual +2.8 dB). Every
guide method scores about F 0.60-0.62 on it. It is kept in the pooled numbers: the guide
can only be as right as the lead stem (see section 7).

### 2.3 vocadito (real solo singing with note annotations)

vocadito (R. Bittner et al., 2021; zenodo.org/records/5578807, **CC BY 4.0**, open
download, no form): 40 solo vocal excerpts in several languages, 817 s, 44.1 kHz, notes
annotated by two annotators. Downloaded to `$JK_BUILD/practice-guide/vocadito-dl`
(58.5 MB zip); only the audio and the note CSVs were extracted (the lyric files were
never extracted or read). Annotator 1 is the reference; annotator 2 against annotator 1
gives the human ceiling. MIR-1K was not used: its annotations are frame pitch, not
notes, and vocadito is licensed explicitly.

**Tuning on the evaluation sets.** Three tunables (speech slope, speech share, note
penalty) were chosen on a coarse grid using the synthetic singer and vocadito, so those
two sets are not held out for the pYIN numbers. The neural trackers go through the same
tuned segmentation; that favours pYIN, whose curves the grid was run on. Section 6.1
measures how much (up to 0.03 F on separated vocals) and why it does not change the
decision.

### 2.4 Real songs (no ground truth)

Lead stems from the files: the three CC end-to-end songs (`$JK_BUILD/e2e-stick/Library`:
`Cortez - Feel (Stripped).jksong`, `coruscate - The Homeless Man.jksong`,
`Jason Miller - Crowdpleaser.jksong`), the public-domain `demo-song.jksong`, and the
owner's song (`dist/Library/Imagine Dragons - It's Time.jksong`, copied to
`$JK_BUILD/practice-guide/owner/` first; SHA-256 of the dist file before and after
equals the 2026-11-19 backup's, `4eb88d781f9deb86...`).

## 3. Neural trackers

Installed in an overlay venv, `$JK_BUILD/practice-guide/venv`, whose `.pth` adds the
Studio venv's site-packages (torch 2.14.1+cu130), so the Studio venv was not touched.
`uv pip install` would have replaced torch with 2.15.0 from PyPI, so each package went in
with `--no-deps` plus the pure-Python pieces it imports.

| Tracker | Version | Installs? | Code licence | Weights licence (bundled in the wheel) | Voicing used |
|---|---|---|---|---|---|
| torchcrepe ("full") | 0.0.24 | yes, with torchaudio 2.11.1 (`--no-deps`), resampy | MIT | MIT (CREPE weights, 87 MB `full.pth` + `tiny.pth`) | periodicity >= 0.21 after a 3-frame median, Viterbi decoder (README recipe). Its A-weighted silence gate at -60 was dropped: it silenced the whole demo-song lead (-18.7 dBFS RMS) |
| PESTO | pesto-pitch 2.0.1 | yes, with torchaudio, omegaconf | **LGPL-3.0** | LGPL-3.0 (648 KB, trained on MIR-1K) | confidence >= 0.5 |
| FCPE | torchfcpe 0.0.4 | yes, with torchaudio, einops, local-attention | MIT | MIT (42 MB `fcpe_c_v001.pt`) | its own threshold 0.006, `local_argmax` |
| SwiftF0 | swift-f0 0.3.0 | yes (onnxruntime already present) | MIT | MIT (172 KB `model.onnx`) | confidence >= 0.5 (its documented rule); runs on the CPU |
| RMVPE | - | **no**: no PyPI package; needs model code copied out of a voice-conversion project plus a separately downloaded weight file | - | - | not evaluated |

Each curve goes through the same Rust note segmentation (`guide_wav --curve`), so only
the tracker differs. Evaluation-only packages: mir_eval 0.8.2 (MIT), matplotlib 3.11.2.

## 4. Accuracy with ground truth

Note metrics are mir_eval's: a note is found when its onset is within 50 ms and its
pitch within 50 cents (F); `F_off` also needs the offset within 20 % of the note or
50 ms. Pitch error: each true note against the guide note covering most of it (at least
30 %), octave errors (11-13 semitones) counted apart. "Time covered": share of the true
notes' time under a guide note within 50 cents (what grading uses). Unpitched: share of
the spoken syllables' time covered by unpitched notes (recall) and share of unpitched
note time that is spoken (precision). All sets pooled.

### 4.1 Synthetic singer alone (365 notes, 126 spoken syllables)

| Method | P | R | **F** | F_off | time covered | pitch err median / p90 (cents) | octave err | onset err median (bias) | unpitched R / P | sung time marked unpitched | notes on breaths |
|---|---|---|---|---|---|---|---|---|---|---|---|
| **Rust pYIN** | 0.970 | 0.981 | **0.975** | 0.973 | 0.959 | 2.0 / 6 | 1 | 12 ms (+10) | 0.98 / 0.87 | 0.0 % | 0 |
| SwiftF0 | 0.940 | 0.984 | 0.961 | 0.953 | 0.968 | 2.5 / 7 | 0 | 9 ms (+8) | 0.90 / 0.91 | 0.0 % | 0 |
| CREPE | 0.893 | 0.981 | 0.935 | 0.924 | 0.972 | 2.7 / 8 | 0 | 12 ms (+1) | 0.84 / 0.86 | 0.0 % | 0 |
| FCPE | 0.894 | 0.967 | 0.929 | 0.926 | 0.964 | 2.0 / 6 | 0 | 10 ms (+1) | 0.86 / 0.77 | 1.0 % | 12 |
| PESTO | 0.929 | 0.926 | 0.927 | 0.914 | 0.948 | 15.8 / 16 | 0 | 13 ms (+13) | 0.75 / 0.97 | 0.1 % | 0 |

Per voice (pYIN F): bass 0.989, tenor 0.977, alto 0.968, soprano 0.968. Recall by
feature (pYIN): vibrato notes 0.96, scooped 1.00, slid into 0.83, fast-run notes 0.99,
several notes on one syllable 0.99. PESTO's pitch error of 15.8 cents is not a
calibration bias: its curve values sit on a grid of thirds of a semitone (on the bass
voice, 3,753 of 3,920 voiced frames lie within 2 cents of 0, 33.3 or 66.7 cents above an
A=440 semitone), so the A=436 notes snap to the A=440 grid. It is probably the wrapper's
decoding setting, not the model; PESTO is LGPL-3.0 and was not a contender either way.

### 4.2 Synthetic singer through the real Studio separation

| Method | P | R | **F** | F_off | time covered | pitch err median / p90 | octave err | onset err median (bias) | unpitched R / P | sung time marked unpitched | notes on breaths |
|---|---|---|---|---|---|---|---|---|---|---|---|
| **Rust pYIN** | 0.859 | 0.888 | **0.873** | 0.863 | 0.860 | 2.1 / 8 | 3 | 12 ms (+10) | 0.90 / 0.79 | 0.9 % | 3 |
| SwiftF0 | 0.833 | 0.904 | 0.867 | 0.859 | 0.875 | 2.8 / 8 | 1 | 9 ms (+8) | 0.83 / 0.78 | 1.5 % | 6 |
| FCPE | 0.832 | 0.899 | 0.864 | 0.851 | 0.881 | 2.4 / 7 | 0 | 11 ms (+2) | 0.86 / 0.72 | 1.6 % | 6 |
| PESTO | 0.875 | 0.822 | 0.847 | 0.839 | 0.805 | 15.8 / 17 | 0 | 15 ms (+15) | 0.54 / 0.62 | 3.7 % | 2 |
| CREPE | 0.693 | 0.904 | 0.785 | 0.778 | 0.881 | 3.1 / 11 | 2 | 11 ms (+2) | 0.43 / 0.78 | 0.4 % | 3 |

Per voice F (pYIN / SwiftF0 / FCPE): bass 0.962 / 0.973 / 0.962, tenor 0.971 / 0.940 /
0.983, alto 0.619 / 0.601 / 0.613 (the separation swap of 2.2), soprano 0.963 / 0.979 /
0.920. Without the alto mix, pYIN is at 0.96-0.97 on separated vocals. Of pYIN's three
notes an octave away from the truth here (the table's rule), one is the alto swap (the
other singer's note at 41.8 s, 12.3 semitones away) and two are real (soprano, see 4.4).

**Tuning offset** (truth -15.86 cents), pYIN per voice: alone -15.75 / -14.98 / -15.60 /
-15.33; separated -16.05 / -14.90 / -14.17 / -15.39: all within 1.7 cents. SwiftF0
-14.6..-15.1, FCPE -15.1..-17.0, CREPE -13.2..-14.7, PESTO -2.4..-4.9.

### 4.3 vocadito (real solo singing, 2237 reference notes)

| Method | P | R | **F** | F_off | time covered | pitch err median / p90 | octave err | onset err median (bias) | sung time marked unpitched |
|---|---|---|---|---|---|---|---|---|---|
| Annotator 2 (human ceiling) | 0.798 | 0.669 | 0.728 | 0.619 | 0.899 | 1.8 / 73 | 1 | 0 ms | - |
| **Rust pYIN** | 0.597 | 0.561 | **0.578** | 0.419 | 0.835 | 6.2 / 97 | 13 | 15 ms (-9) | 4.1 % |
| SwiftF0 | 0.611 | 0.531 | 0.568 | 0.424 | 0.800 | 8.0 / 138 | 2 | 13 ms (-10) | 5.6 % |
| FCPE | 0.562 | 0.533 | 0.547 | 0.409 | 0.840 | 7.7 / 105 | 3 | 23 ms (-21) | 4.2 % |
| CREPE | 0.541 | 0.540 | 0.541 | 0.385 | 0.843 | 9.3 / 106 | 4 | 23 ms (-20) | 2.2 % |
| PESTO | 0.599 | 0.340 | 0.434 | 0.310 | 0.624 | 14.7 / 205 | 6 | 17 ms (-13) | 14.8 % |

Real singing is far harder to cut into notes than the synthetic singer, and even two
people agree only at F 0.73. The pictures (`pictures/voc-1..3.*.png`) show the misses
are mostly where an onset is placed 50+ ms differently (slides, scoops) or a curved
note is cut once more or once less, not wrong pitches: 83.5 % of the annotated note
time is covered by a pYIN note within 50 cents (annotator 2: 89.9 %).

### 4.4 Octave errors and the independent check

The spectral octave check (section 1.1), measured on pYIN guides: on the synthetic
singer it flags the one real octave error (a soprano note whose second harmonic is 14 dB
above its fundamental, which YIN read an octave high) and no correct note; through
separation it flags both real ones (the same soprano note and one more; the alto-swap
note is another singer, not an octave error) and no correct note; on vocadito, with the
table's rule (each true note against the guide note covering most of it, at least 30 %),
pYIN has 13 octave errors and the check flags 10 of them, and 15 of the 1,722 notes
within 50 cents (0.9 %). Flagged notes become low confidence, so they are not graded.
pYIN still makes more octave errors than the neural trackers on vocadito (13 vs 2-4 of
about 2,100 notes before the check; after it, 3 unflagged, 0.1 %).

### 4.5 Confidence is meaningful

Share of pYIN notes that are right (within 50 cents of the true note covering most of
them), by confidence: vocadito < 0.3: 26 % (103 notes), 0.3-0.5: 51 % (128), 0.5-0.7:
64 % (318), 0.7-0.85: 87 % (618), >= 0.85: 93 % (932). Separated synthetic: 0.3-0.5: 0 %
(12), 0.5-0.7: 45 % (22), 0.7-0.85: 79 % (53), >= 0.85: 97 % (289). So the "low confidence" cut at 0.5 drops mostly wrong notes.

## 5. Real songs (no ground truth)

Counts per guide (pYIN, the chosen method; the other methods are in `results.json`).
Low confidence = below 0.5 (drawn dim, not graded). Agreement = note F of pYIN against
SwiftF0 (50 ms, 50 cents).

| Song (file) | Length | Notes | Pitched | Unpitched | Low conf. | Octave flags | Tuning (strength) | Agreement with SwiftF0 / FCPE / CREPE |
|---|---|---|---|---|---|---|---|---|
| Cortez - Feel (Stripped) | 240.6 s | 503 | 462 | 41 | 217 | 22 | -0.7 c (0.91) | 0.60 / 0.51 / 0.61 |
| coruscate - The Homeless Man | 245.7 s | 679 | 210 | 469 | 116 | 5 | -18.5 c (0.16) | 0.32 / 0.35 / 0.29 |
| Jason Miller - Crowdpleaser | 192.8 s | 593 | 165 | 428 | 83 | 0 | +27.8 c (0.11) | 0.33 / 0.36 / 0.29 |
| demo-song (public domain, generated) | 40.0 s | 42 | 42 | 0 | 0 | 0 | 0.0 c (1.00) | 1.00 / 1.00 / 1.00 |
| owner's song (It's Time) | 240.8 s | 435 | 397 | 38 | 64 | 12 | -2.0 c (0.79) | 0.66 / 0.70 / 0.65 |

Looked at (`$JK_BUILD/practice-guide/pictures/song-*.pyin.png`: guide bars over the
pitch curve, SwiftF0's notes as outlines; `*.swiftf0.png` the other way round):

- **Feel (Stripped)** and **the owner's song**: the bars follow the singing; held notes
  are one bar, runs are several; pYIN and SwiftF0 draw nearly the same notes. Feel's
  many low-confidence notes are short, breathy notes in a stripped acoustic mix (4.5 says
  such notes are usually wrong, so dimming them is honest; it does mean Feel is graded
  on about half its notes).
- **The Homeless Man** and **Crowdpleaser** are mostly rapped: the curve is continuous
  speech-like glides and most of it is marked unpitched by every method (pYIN 69 % and
  72 % of notes; the neural trackers more). Their tuning strength (0.16, 0.11) says
  there is no consistent tuning to estimate; the guide should then fall back to
  A = 440 (Stage 2: use the tuning only above a strength of about 0.3). The low
  agreement between methods there is between their pitched fragments of rap.
- **Owner's song, open item**: between about 213.5 and 215.7 s pYIN draws five notes
  at MIDI 76-78 where SwiftF0, FCPE and CREPE all draw 64-66, an octave lower; two
  shorter places elsewhere (46.3 s at MIDI 80, 158.6 s at 74-76) look the same. In the
  current build the octave check flags four of the five notes at 214.2-215.7 s and all
  three at 158.6-159.3 s (so they are dim and not graded), and the 46.3 s note already
  has confidence 0.09. What remains graded is one note, 213.47-214.22 s (MIDI 76.1,
  confidence 0.99, check "ok"). Which octave is right is not settled: in the note
  spectra the strongest partial is at pYIN's pitch (about 664 Hz), the neural trackers'
  fundamental (about 332 Hz) is 25-28 dB below it, and the odd multiples of 332 Hz are
  30-42 dB down, which looks like a strong voice at pYIN's octave plus a weak lower layer
  rather than a voice at 332 Hz. Three trackers agreeing is not proof either. It needs a
  listen, and is carried into Stage 2 (section 7).
- **demo-song**: 42 notes, exactly the generated melody (MIDI 60-69), every method agrees.

## 6. Speed, size and the decision

Rust builder, release build, one thread, fastest of 5 runs on the 240.6 s lead vocal of
Feel (Stripped) (`speed.json`): **1.17 s** in all (decimation 0.087 s, pYIN 0.784 s,
notes + octave check 0.300 s), 0.024 s to read the WAV. It is about 200 times faster
than the song. Memory is dominated by the Viterbi back-pointers, about 24 MB for 4
minutes by construction (24,057 frames x 504 states x 2 bytes; not measured). The
synthetic voices (61-65 s) took 0.25-0.35 s each.

**M1 estimate (not measured):** the builder is single-threaded scalar code; the M1's
single-core speed is about 1.4-1.5 times below the 9700X's (published Geekbench 6
single-core scores, about 2,350 vs 3,400), so about **1.7-2 s** for a 4-minute song on
the MacBook Air.

Neural trackers on the same 4-minute vocal, after a warm-up run (model load excluded):

| Tracker | GPU (RTX 5070 Ti) | CPU (8 threads) | Weights |
|---|---|---|---|
| CREPE full | 3.71 s | 154.6 s | 87 MB |
| PESTO | 8.04 s | 6.49 s | 0.6 MB |
| FCPE | 0.30 s | 1.77 s | 42 MB |
| SwiftF0 | - (CPU onnxruntime) | 0.69 s | 0.2 MB |

All 53 vocals (2,278 s of audio) on the GPU: CREPE 33.4 s, PESTO 61.4 s, FCPE 4.2 s;
SwiftF0 7.7 s on the CPU.

**The rule** (WO2 section 6): "If the plain Rust builder is close to the best neural one
on your benchmark, use it alone, everywhere." The best neural tracker overall is SwiftF0
(FCPE is close). The numbers that decided it:

| | Rust pYIN | best neural |
|---|---|---|
| F, synthetic alone | **0.975** | 0.961 (SwiftF0) |
| F, synthetic through separation | **0.873** | 0.867 (SwiftF0), 0.864 (FCPE) |
| F, vocadito | **0.578** | 0.568 (SwiftF0) |
| Time covered within 50 cents (alone / separated / vocadito) | 0.959 / 0.860 / 0.835 | 0.972 / 0.881 / 0.843 (best of each, CREPE) |
| Median pitch error alone / separated (cents) | **2.0 / 2.1** | 2.0 / 2.4 (FCPE) |
| Tuning error (synthetic, worst voice) | 1.7 cents | 1.3 cents (SwiftF0) |
| Unpitched recall / sung time lost to "unpitched", separated | **0.90 / 0.9 %** | 0.86 / 1.6 % (FCPE) |
| Octave errors before the check (alone / separated / vocadito) | 1 / 2 real / 13 | 0 / 0-2 / 2-4 |
| Time for a 4-minute song | 1.17 s, one CPU core, any computer | 0.30 s GPU / 0.69-1.77 s CPU, Studio only |
| Size added to the stick | 0 | 0.2-87 MB plus a runtime on the Mac |

At the default tuning the Rust builder is ahead on note F everywhere, within 1-2 points
on time covered and 0.4 cents on tuning, and clearly behind only on octave errors (0.6 %
of vocadito notes before the check, 0.1 % after). With the segmentation retuned for the
neural curves (6.1) the best neural tracker is up to 0.03 F ahead on separated vocals and
level on real singing. Either way the two are close, so under the rule it is **used
alone, on Windows and the Mac**: one implementation, the
same guide on both systems, and the Mac can add a guide to an old song without the
Studio (WO2 4.3). No model is listed in MODELS.md for the guide.

### 6.1 Sensitivity to the segmentation's tuning (independent verification)

The note segmentation's defaults were tuned on pYIN curves. Re-running it on the cached
curves with two tunables changed (`guide_wav --set note_penalty=... --set min_note_s=...`,
scored with an independent Hungarian matcher that reproduces mir_eval's F to three
decimals at the defaults):

| note_penalty / min_note_s | pYIN alone / sep / vocadito | SwiftF0 alone / sep / vocadito | FCPE alone / sep / vocadito |
|---|---|---|---|
| 16000 / 0.04 (default) | **0.975** / 0.873 / 0.578 | 0.961 / 0.867 / 0.568 | 0.929 / 0.864 / 0.547 |
| 16000 / 0.07 | 0.973 / 0.873 / 0.549 | 0.966 / **0.905** / 0.553 | 0.940 / 0.884 / 0.522 |
| 32000 / 0.04 | 0.964 / **0.877** / 0.578 | 0.963 / 0.899 / **0.581** | 0.935 / 0.897 / 0.548 |
| 32000 / 0.07 | 0.968 / 0.875 / 0.565 | 0.963 / 0.902 / 0.570 | 0.941 / **0.901** / 0.529 |
| 8000 / 0.04 | - / 0.828 / 0.562 | - / 0.828 / 0.555 | - / 0.810 / 0.532 |

Each method at its own best setting: alone pYIN 0.975 vs SwiftF0 0.966; separated pYIN
0.877 vs SwiftF0 0.905; vocadito pYIN 0.578 vs SwiftF0 0.581. So the neural trackers'
noisier frame-to-frame curves want a stronger cut penalty, and with it SwiftF0 is about
0.03 F better on separated vocals and equal on real singing. That is
still "close", and it does not outweigh one implementation on both systems with no model,
so the decision stands; Stage 2 may try `note_penalty` 32000 for pYIN too (separated
0.877, vocadito unchanged, synthetic alone 0.964).

## 7. Limits and what Stage 2 inherits

- Real singing: note F 0.58 against a human-vs-human 0.73 on vocadito. Most misses are
  onset placement on slides and scoops and the cutting of curved notes; grading uses
  time on pitch, where 84 % of the annotated note time is covered within 50 cents.
- Octave: pYIN's weak spot. The spectral check catches most errors (10 of 13 on
  vocadito) and four of the five notes of the owner's-song stretch in section 5, whose
  right octave is itself unsettled. Stage 2 options, cheapest first: a
  phrase-level octave consistency check (a note an octave away from both neighbours and
  from the phrase's median is suspect), then a second look with the spectral check at
  that note's lower octave.
- Low confidence: 47 % of the notes on a breathy stripped vocal (Feel). Honest per 4.5,
  but Stage 2 should look at whether the voicing probability under-rates breathy notes.
- Tuning on rap songs is noise (strength 0.11-0.16): use it only above a strength of
  about 0.3.
- Rapped lines are not yet consistent: inside them, syllables with a flat contour still
  come out as short pitched notes (for example Crowdpleaser around 80-82 s and 138-150 s).
  Stage 2 should decide spoken lines over a whole lyric line (by overlap with its words,
  never moving a note).
- Separation: when another voice is as strong as the lead (the alto mix), the "lead"
  stem can swap singers, and the guide follows the stem. This is a Studio limit, not a
  guide one.
- The tunables were chosen on the evaluation sets (2.3).

## 8. Reproduce

```bash
cd jukeoke && source scripts/devenv.sh
CARGO_TARGET_DIR="$JK_BUILD/target-wo2-guide" cargo test -p jk-pitch --release guide
CARGO_TARGET_DIR="$JK_BUILD/target-wo2-guide" cargo build --release -p jk-pitch --example guide_wav
# overlay venv: uv venv $JK_BUILD/practice-guide/venv (CPython 3.12), a .pth running
# site.addsitedir(<studio-venv>/Lib/site-packages), then uv pip install --no-deps
# torchcrepe pesto-pitch torchfcpe swift-f0 torchaudio==2.11.1 omegaconf pyyaml einops
# local-attention hyper-connections torch-einops-utils antlr4-python3-runtime resampy
# flatbuffers protobuf packaging mir_eval matplotlib pillow contourpy cycler fonttools
# kiwisolver pyparsing python-dateutil six
$JK_BUILD/practice-guide/venv/Scripts/python.exe -I studio/bench/practice_guide_bench.py all
```

vocadito is expected unzipped (Audio, Annotations/Notes) in
`$JK_BUILD/practice-guide/vocadito`.

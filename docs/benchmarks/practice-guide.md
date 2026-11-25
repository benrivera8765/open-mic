# Practice mode: the melody guide (WO2 Stage 0)

Work Order 2 sections 4.1, 6 ("Guide builder") and 9 ("Guide accuracy", "Guide on real
songs"). Measured 2026-11-19 on the build PC (Ryzen 7 9700X, RTX 5070 Ti, Windows 11).
Stage 7 merges this file into `docs/benchmarks/practice.md`. Stage 2 (the guide in the song
file, adding guides to existing songs, the octave open item) is the last section.

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

## Stage 2: the guide in the song file, every song gets one

Measured 2026-11-19 on the build PC (Ryzen 7 9700X, NVMe SSD, Windows 11), with other
workstreams' benchmarks running at the same time (timings are therefore upper bounds).
Evidence in `$JK_BUILD/practice-stage2/` (outside the repository): `measure/results.json`,
`octave/results.json` and the guides per setting, `conformance.log`, `oldrelease/`,
`tests/`, `ui-screenshots/`.

### S2.1 What was built

| Piece | Where |
|---|---|
| Format 1.1 in Rust: guide and curve types, curve codec, section 9 checks, guide state (none / ready / outdated / stale / newer), word-reference cache rule | `crates/jk-format/src/guide.rs` |
| The raw-copy rewrite of SONG_FORMAT 7.1 (prepare / finish split for the deferred swap, simulated interruptions, cancel) | `crates/jk-format/src/guide_write.rs` |
| Practice slot data (types, number-precision rules, best / recent, the size rule `save_practice`) | `crates/jk-format/src/practice.rs` |
| `verify` additions (guide, curve, stale, `--deep` source SHA-256, slot `practice`) | `crates/jk-format/src/verify.rs` |
| The same in Python, written independently | `studio/jukeoke_studio/jksong.py` (the "1.1" part) |
| Word references by the overlap rule (pure) | `crates/jk-pitch/src/guide/words.rs` |
| Phrase-context octave check (S2.4) | `crates/jk-pitch/src/guide/octave.rs` (`context_check`) |
| Build from a song file + write (shared by the CLI and the app) | `crates/jk-guide` (new crate) |
| `jukeoke-cli guide add|build`, `guide show|info`, `verify --deep` | `crates/jk-cli/src/main.rs` |
| App core: guide queue, `Settings/guide-queue.json`, deferred swap, Studio `guide` stage, commands and events | `app/src-tauri/src/guide.rs`, `studio.rs`, `core.rs`, `commands/guide.rs` |
| Studio `make --guide auto|off|require [--jukeoke-cli PATH]` | `studio/jukeoke_studio/__main__.py` |
| Library: "Practice ready / not ready", "Add practice data", "Add practice data to all songs" with progress and Stop (both looks) | `app/ui/src/components/Library.tsx`, `state/app.ts`, `api/*` |
| Benchmarks | `studio/bench/practice_guide_stage2.py` (S2.3), `studio/bench/practice_guide_octave.py` (S2.4) |

### S2.2 Proof: byte identity, interruptions, both implementations, the previous release

- **Conformance** (`bash tests/conformance/run.sh`, steps 8-10 new, PASS): Python adds a
  guide by the raw copy and Rust verifies it (`--strict --deep`), recomputes the 20 word
  references by its own rule (identical) and finds every other chunk and both slot regions
  with the same SHA-256 at the same offsets; the Rust builder adds a guide (17 notes) and
  Python does the same checks the other way; Python and Rust compute the same SHA-256
  fingerprint of the result. Each writer was stopped at 8 points of 7.1 (after the
  placeholder, after the slots, mid-chunks, before the TOC, before the header, before the
  second slot copy, before the rename, a temporary file cut to half): both readers found
  the old file byte for byte, valid, and listed no temporary file.
- **Rust tests** (`crates/jk-format/tests/guide.rs`, 12): every other chunk and both slots
  byte-identical after adding and after replacing (the guide keeps its place); an invalid
  slot is copied raw; 8 interruption points; cancel; a settings save between the
  temporary file and the swap is kept; a whole-file rewrite of the original meanwhile stops
  the swap (`ChangedOnDisk`, temporary file deleted); a damaged chunk stops the rewrite;
  verify finds stale guides (also `--deep`), overlapping notes, a damaged curve, a newer
  `guide_version`; a 1.0-style rewrite keeps the guide; practice data shrinks by the size
  rule and never rewrites the file. Python twin: `studio/tests/test_jksong_guide.py` (21).
- **The previous release** (commit ef62188, built from a `git worktree` under
  `$JK_BUILD/practice-stage2/old-ef62188`): on the demo song with a guide added by the new
  CLI and a `practice` value saved in its slot (header minor 1, manifest unchanged):
  - old `jukeoke-cli verify --strict`: PASS, 0 errors, 0 warnings; old `info` lists the two
    guide chunks among the chunks and nothing else changes;
  - old `jukeoke-cli render` at keys 0, +2 and -5: WAV files byte-identical to the renders
    of the same song without a guide;
  - the old Python slot save (key +2) keeps the `practice` value and both guide chunks; the
    new `verify --strict --deep` then still passes;
  - **the old app** (`jukeoke.exe` built from ef62188, driven by `real-app-check.mjs` on
    the null device, master 0): lists the song, opens it, plays (clock 30.7 events/s, UI
    clock within 8.5 ms of the engine), the key change reaches the core; afterwards its slot
    save had kept the `practice` value and the guide (new `verify --strict --deep` PASS).
- **App core** (`app/src-tauri/tests/guide_queue.rs`, 9, fake engine that holds the loaded
  file open as the real one does; `tests/real_engine.rs`, 1 new, real engine on the null
  device): `guide_build` keeps every other chunk and both slots byte-identical; the playing
  song keeps playing (no unload, guide used from memory, `pending_write`) and is swapped
  when paused, then loaded again at the same position (real engine: 0 underruns while the
  guide was built and written beside playback, reloaded within 0.1 s of where it was,
  plays on); a song loaded but paused is released, swapped and reloaded at once with the
  settings save made just before kept; a damaged chunk gives `failed` / `guide_damaged`
  with the guide usable from memory and the song still plays; no lead vocal gives
  `failed` / `guide_no_lead` and nothing is queued; "Add practice data to all songs" with
  `guide://batch` (damaged file skipped); the queue survives a restart
  (`guide-queue.json`, a song that is gone is dropped); cancel; a new song from the (fake)
  Studio shows the `guide` stage (running, overall 0.95..1.0, "Making the practice data")
  and ends `done` with `guide.state` ready.

### S2.3 Adding guides to existing songs (`jukeoke-cli guide add`, builder 0.2.0)

Fresh copies (`studio/bench/practice_guide_stage2.py`); the originals' SHA-256 unchanged
(dist/Library included: `4eb88d78...` before and after). "Build" is the guide's
`provenance.seconds` (hashing and decoding the lead stem, pYIN, notes, word references);
"add" is the whole command (build + raw-copy rewrite + checks), fastest / median of 3
runs (the first adds, two more with `--force`). Build and add are given for two sessions,
"session 1 / session 2" (fastest of 3 each); other workstreams' benchmarks loaded the PC
more during session 2 (sizes and counts are from session 2, identical in both).

| Song | Length | File | Build | Add | `guide` | `guide.curve` | File grows | Notes / pitched / unpitched / low conf. / octave flags / graded | Spoken spans | Notes with a word | Tuning (strength) -> used | Byte-identical |
|---|---|---|---|---|---|---|---|---|---|---|---|---|
| Cortez - Feel (Stripped) | 240.6 s | 111.1 MB | 1.26 / 1.72 s | 1.42 / 2.38 s | 66.0 KiB | 282.0 KiB | 352.2 KiB (0.32 %) | 503 / 462 / 41 / 217 / 22 / 245 | 37 | 427 | -0.7 (0.91) -> -0.7 | all 6 chunks + 2 slots, 3 runs |
| coruscate - The Homeless Man | 245.7 s | 142.3 MB | 1.04 / 1.52 s | 1.24 / 1.69 s | 95.0 KiB | 288.0 KiB | 384.2 KiB (0.28 %) | 679 / 210 / 469 / 116 / 5 / 94 | 86 | 638 | -18.5 (0.16) -> +0.0 | all 6 chunks + 2 slots, 3 runs |
| Jason Miller - Crowdpleaser | 192.8 s | 109.7 MB | 0.85 / 1.58 s | 1.01 / 1.70 s | 82.8 KiB | 226.0 KiB | 312.2 KiB (0.29 %) | 593 / 165 / 428 / 83 / 0 / 82 | 61 | 572 | +27.8 (0.10) -> +0.0 | all 6 chunks + 2 slots, 3 runs |
| owner's song | 240.8 s | 140.4 MB | 1.37 / 1.95 s | 1.56 / 2.44 s | 58.1 KiB | 282.2 KiB | 344.2 KiB (0.25 %) | 435 / 397 / 38 / 65 / 13 / 332 | 35 | 401 | -2.1 (0.79) -> -2.1 | all 6 chunks + 2 slots, 3 runs |
| demo-song | 40.0 s | 2.4 MB | 0.17 / 0.18 s | 0.20 / 0.21 s | 6.3 KiB | 46.9 KiB | 56.2 KiB (2.37 %) | 42 / 42 / 0 / 0 / 0 / 42 | 0 | 42 | +0.0 (1.00) -> +0.0 | all 7 chunks + 2 slots, 3 runs |

Every song: `jukeoke-cli verify --strict --deep` PASS, 0 warnings, after the third run; every
other chunk (manifest, stems, lyrics, cover, unknown) and both slot regions byte-identical,
at the same offsets, after each of the 3 runs.

On a USB stick the rewrite is one more copy of the file (110-142 MB): at 20-40 MB/s write
speed that is 3-7 s (not measured: no stick was attached; WO2 trap 10). The build itself
reads the lead stem only (20-40 MB). The temporary file needs that much free space.

### S2.4 The Stage 0 open item: octave errors on the owner's song

Stage 0 left one note graded at the wrong octave with high confidence in the owner's song:
213.47-214.22 s at MIDI 76.1 ("ok", confidence 0.99), at the head of a run (214.22-215.68 s,
MIDI 76-78) that the spectral check flagged `maybe_high`, while the melody around it sits
at MIDI 62-66 (and SwiftF0, FCPE and CREPE all draw 64-66 there).

**Change (builder 0.2.0): a phrase-context check after the spectral one**
(`octave::context_check`, the "run" rule): an unflagged pitched note that touches (gap at
most 0.1 s) a neighbour the spectral check flagged in one direction, lies within 2.5
semitones of it, and whose context (the unflagged pitched notes within 2 s, at least two
and 0.3 s of them, duration-weighted median) is 9.5 or more semitones away in that
direction with the note's other octave within 3.5 semitones of it, is flagged too
(confidence x 0.4, so dim and not graded). Repeated until nothing changes. A second rule
("isolated": a short note whose two touching neighbours both sit at its other octave) was
measured and **not** enabled (`octave_context=2` turns it on): it changed nothing on the
evaluation sets and flagged 2 more notes on Feel (Stripped) and 1 more on the owner's song
whose right octave is unknown.

Before / after on the Stage 0 benchmark subset (`studio/bench/practice_guide_octave.py`;
"before" = `--set octave_context=0`, i.e. builder 0.1.0's flags):

| Set | Note F before / after | Octave errors | flagged before / after | still graded before / after | correct notes flagged before / after |
|---|---|---|---|---|---|
| Synthetic singer alone (365 notes) | 0.975 / 0.975 | 1 | 1 / 1 | 0 / 0 | 0 / 0 (of 364) |
| Synthetic singer, separated | 0.873 / 0.873 | 3 | 2 / 2 | 1 / 1 | 0 / 0 (of 333) |
| vocadito (2237 notes) | 0.578 / 0.578 | 13 | 10 / 10 | 2 / 2 | 10 / 10 (of 1693) |

Nothing moved on the synthetic singer, the separated synthetic singer or vocadito: the
rule only fires next to a flagged run, which those sets do not have in the shape it looks
for. On the real songs it adds exactly one flag: the owner's note at 213.475 s. After the
change every note of the three suspect places of the owner's song is out of the grade:
46.26 s (MIDI 80.1) has confidence 0.09, 158.61-159.31 s (MIDI 74-76) and 213.48-215.68 s
(MIDI 76-78) are flagged `maybe_high`. Which octave the singer really used there is still
not settled (in the note spectra the strongest partial is at the guide's pitch, about
664 Hz; three neural trackers say an octave lower): it needs a listen. Either way the
lane draws those bars dim and the grade does not count them, so a wrong octave there can
cost the singer nothing. CC songs: unchanged (22 / 5 / 0 flags).

### S2.5 The app and the UI

- `SongSummary.guide` (and `SongDetail.summary.guide`) carry `GuideStatus`; the core
  overlays the queue's live state (queued / building with progress / in memory waiting for
  the swap / failed with reason) over what the file says. `practice_best` comes from the
  slot. The library scan reads the guide JSON but not the curve (cached by size and time).
- Commands `guide_status`, `guide_build`, `guide_build_all`, `guide_cancel`, `guide_queue`;
  events `guide://status` (progress at most 5 a second) and `guide://batch`.
- One build at a time on a below-normal-priority thread. A song file another program holds
  is tried again every 3 s, 20 times, then `guide_in_use`. Leftover temporary files older
  than an hour next to a song are deleted before its build. Lyrics, metadata and delete
  take the same song-file lock as the swap.
- Library, both looks: a "Practice ready" / "Practice not ready" / "Adding practice data
  n %" badge (plain card) or note (skinned strip, with `data-practice` on the row); "Add
  practice data" (plain) / "+ Practice" (skin plate, on hover like the bin) on songs
  without it, "Try again" with the reason after a failure; "Add practice data to all songs"
  in the toolbar with "Adding practice data: n of N" and Stop. Screenshots:
  `$JK_BUILD/practice-stage2/ui-screenshots/library-1440x900.png` (skin) and
  `plain-library-1440x900.png`. Vitest `src/components/library-practice.test.tsx` (12: both
  looks, and the Studio job's new "Practice data" stage after Save, with the reason and
  "Add practice data again" when the guide failed; a failed guide never fails the job).

### S2.6 Readings of the contracts (proposed wording; the contracts were not edited)

Where the Stage 1 contracts left a choice or did not fit, the code follows the reading
below; each wants a line in the contract.

1. **CLI names.** STUDIO_PROTOCOL 12.3 says `guide add` / `guide show`; the Stage 2 brief
   said `guide build` / `guide info`. Both work (`build` and `info` are visible aliases).
   Proposed: "`jukeoke-cli guide add SONG... [--force] [--json]` (alias `build`)" and
   "`guide show SONG [--json]` (alias `info`)".
2. **`counts.low_confidence`** counts pitched notes only (the example's 397 - 64 = 333
   graded says so). Proposed in 10.1: "`low_confidence`: pitched notes whose confidence is
   below `low_confidence`".
3. **Word rule, zero-length words.** With "at least 50 % of the word's length", a word of
   length 0 would take every note it touches with an overlap of 0. Proposed in 10.4: "kept
   only when that overlap is positive and at least 30 % of the note's length or at least 50 %
   of the word's length".
4. **Header minor on a rewrite.** 7.1 says "minor 1"; a file from a newer minor keeps its
   number (`max(original, 1)`), since every byte of that version's data is copied. Proposed:
   "the final header (`minor` = the larger of the original's and 1, ...)". A file whose
   header grew (`header_length` above 128) is not rewritten (a 1.1 writer cannot copy it).
5. **Severity in section 9.** Unknown `kind`, `octave` and span kinds are warnings (readers
   must accept them by 10.1); counts that disagree with the notes are warnings
   (informational). `range` is `null` when there are no pitched notes and `words` is `null`
   for an instrumental (no lyrics). Proposed: say so in 9 and 10.1.
6. **Spoken spans (builder 0.1.0 and 0.2.0)**: runs of unpitched notes with gaps under
   0.3 s (the builder's phrase gap). Proposed in 10.1 "Spans": "builder 0.x writes one
   `spoken` span per run of unpitched notes less than 0.3 s apart".
7. **7.1 steps 3-4 with a debounced settings save.** A slot save can land between the second
   slot copy and the final look at the original; the writer then copies the slots again
   (three tries) instead of giving up. Proposed: "if only the slot regions changed, step 3
   is repeated (at most three times)". Outside the app (the command line) nothing holds the
   song's write lock: `jukeoke-cli guide add` must not be run on a stick the app has open.
8. **A guide built but not written** (read-only stick, full stick, file in use): the status
   is `failed` with `error_code` and `reason`, and `in_memory: true` (usable this session).
   Proposed in APP_API 12.3: "`failed` with `in_memory` true: built, could not be saved".
9. **Builder version.** 0.2.0 adds the phrase-context octave check (S2.4); SONG_FORMAT 10.1
   says "x 0.4 in builder 0.1.0" and 10.5 describes 0.1.0. Proposed: "x 0.4 in builders
   0.1.0 and 0.2.0" and, in 10.5, "0.2.0 then flags a note left ok inside a run the spectral
   check flagged when the melody around it is an octave away". New tunables in
   `provenance.parameters`: `octave_flag_factor`, `octave_context`, `octave_context_s`,
   `octave_run_gap_s`, `octave_isolated_max_s`.
10. **`make` lookup order.** STUDIO_PROTOCOL 12.3 names the folder above the Studio runtime;
   `make` also looks in the Studio folder itself and in `<--root>/Windows`. Proposed: add
   "then `jukeoke-cli.exe` in the Studio folder".

### S2.7 Reproduce

```bash
cd jukeoke && source scripts/devenv.sh
export CARGO_TARGET_DIR="$JK_BUILD/target-wo2-stage2"
cargo test -p jk-format -p jk-guide -p jk-cli -p jukeoke && cargo test --release -p jk-pitch
$JK_BUILD/studio-venv/Scripts/python.exe -m pytest studio/tests/test_jksong.py studio/tests/test_jksong_guide.py
bash tests/conformance/run.sh
cargo build --release -p jk-cli -p jk-pitch --example guide_wav
$JK_BUILD/studio-venv/Scripts/python.exe -I studio/bench/practice_guide_stage2.py
$JK_BUILD/practice-guide/venv/Scripts/python.exe -I studio/bench/practice_guide_octave.py
(cd app/ui && npm run build && npm test)
```

## Builder 0.3.0: choosing each note's octave (owner, 2026-11-23)

**Why.** In "DragonForce - Seasons (Acoustic Version)" the lane drew "sunlight will shine" at
F#2 / C#2 / B1 under an E4 line. The stored notes sat exactly 24 semitones under the sung
ones (F#2 between F#4s, C#2 by C#4), many with confidence 0.8-0.93 and octave `ok`:
60 notes, 10 % of the singing, 18+ semitones from the melody round them. In this acoustic
mix the guitar leaking into the separated vocal makes the signal repeat at a lower
note's period; the spectral check (section 4.4) looks from the claimed pitch down, finds
that leak, and says "ok". The same kind of runs, an octave or two off, are in most of the
20 library songs (stored: 105 notes 18+ semitones off, 306 leaps over an octave between
notes under 0.6 s apart).

**Change** (`guide::choose_octaves`, before the spectral and phrase checks):

- `OctaveChecker::support`: for candidate pitches (the note's own, +-12, +-24), the mean
  level of the candidate's harmonics minus the mean level half-way between them, both
  only in the voice's band (150 Hz - 4 kHz; partials 30 dB under the strongest count as
  absent), so a bass or guitar note under the singer cannot vote.
- Two Viterbi passes over the pitched notes choose a shift per note: one from the
  spectrum (16 per dB short of the best candidate, x note weight) with the melody, one
  from the melody alone (leaps: 0.6 per semitone plus the square of the excess over a
  fifth, fading out across 0.6-2.5 s rests; range: twice the squared excess over 13
  semitones from the local centre). **A note moves only where both choose the same
  shift**; the curve under it moves with it. Local centres use only notes within 15
  semitones of the song's main register (the 15-semitone band holding the most singing),
  so a run two octaves off cannot drag them.
- The same melody-only alignment runs in every reader (jk-guide singable), so guides
  from older builders are corrected in the lane without rebuilding.

**Why both must agree.** The spectral weight was swept on the benchmark subset
(`studio/bench/practice_guide_choose.py`): spectrum-led choices fixed the most vocadito
errors (13 -> 2 at weight 8) but pulled a clean tenor's and bass's real high notes down
an octave (synthetic 1 -> 3 errors), and in "Seasons" the guitar under the voice pulled a
correct, confident Bb4 down to Bb3. Weight 28 got worse everywhere (vocadito 15). With
agreement required no correct note moved on any set.

| Set (`practice_guide_choose.py`) | Octave errors 0.2.0 -> 0.3.0 | Still graded | Correct notes | Note F |
|---|---|---|---|---|
| Synthetic singer alone (365 notes) | 1 -> 0 | 0 -> 0 | 364 -> 365 | 0.975 -> 0.978 |
| Synthetic singer, separated | 3 -> 1 | 1 -> 1 | 333 -> 335 | 0.873 -> 0.876 |
| vocadito (2237 notes) | 13 -> 6 | 2 -> 1 | 1693 -> 1699 | 0.578 -> 0.581 |

Real songs, rebuilt copies (`examples/singable_stats`; "lane" = what the session draws and
grades after jk-guide singable):

| Song | Stored notes 18+ off, 0.2.0 -> 0.3.0 | Stored leaps > 12 | Graded (stored) | Graded (lane) | Notes moved (up / down) |
|---|---|---|---|---|---|
| DragonForce - Seasons (Acoustic) | 60 -> 39 | 63 -> 42 | 393 -> 395 | 328 -> 328 | 41 (39 / 2) |
| Celine Dion - My Heart Will Go On | 3 -> 0 | 15 -> 8 | 347 -> 353 | 250 -> 254 | 11 (2 / 9; 7 flagged notes resolved) |
| Tal Bachman - She's So High | 0 -> 0 | 8 -> 3 | 362 -> 362 | 282 -> 282 | 3 (3 / 0) |

No confident note (>= 0.7) moved down in any of them. Build time unchanged (1.35-1.93 s).
Version 0.3.0: existing guides read as "outdated" (still used for practice); Library >
Add practice data, or `jukeoke-cli guide add --force`, rebuilds them (only the guide
chunks are rewritten).

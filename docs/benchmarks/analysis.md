# Key and tempo analysis benchmark (Studio step 5)

Status: measured on this machine, 17 November 2026, with `studio/bench/alignment_bench.py keys` (raw JSON: `$JK_BUILD/eval/jamendolyrics/_work/keys_results_final.json`). Code: `studio/jukeoke_studio/analyse.py`. Tests: `studio/tests/test_analyse_key.py`.

## Summary
* Key: template matching of a song-level chromagram (librosa CQT, log-compressed, below 65 Hz ignored) against the Albrecht-Shanahan major/minor profiles. Exact key (tonic and mode): 100 % (24/24) on unambiguous synthetic cadences (unit test); 66.7 % on 96 synthetic pop progressions (3 of the 8 progression types are genuinely ambiguous); 80.0 % / 58.9 % on real guitar accompaniment (GuitarSet comp, dev / held-out players); 42 % / 33 % on guitar solo melodies. No openly licensed full-band annotated set was found, so full songs are not measured. Treat the detected key as a suggestion; the saved and applied value is the shift in semitones.
* Tempo: librosa onset autocorrelation (mean over bands). Within 4 %: 60.4 % synthetic (81.2 % allowing x2, x1/2, x3, x1/3); 55.6-67.8 % on GuitarSet comp (guitar only). Informational only.
* Licences: librosa ISC, numpy/scipy BSD, soxr LGPL-2.1. No AGPL (Essentia) and no non-commercial models (madmom's key CNN is CC BY-NC-SA).
* describe_shift({'tonic':'A','mode':'major'}, 2) == 'A major → B major (+2)': conventional spelling, optional Unicode ♭/♯, 'original key' at 0, '+2 semitones' when the key is unknown.

## Method
1. Key from other + backing_vocals; tempo from drums (or the whole instrumental if drums are silent). Mono, soxr VHQ to 22.05 kHz for analysis only.
2. CQT from C1, 7 octaves x 36 bins, tuned by librosa estimate_tuning; per-bin log(1 + 10|X|); octave below C2 zeroed; folded to 12 bins and summed.
3. Pearson correlation with the 24 profile rotations; confidence = softmax weight (beta 30) of the best.
4. Tempo: onset_strength(aggregate=np.mean), then librosa.feature.tempo, folded into 60-200 BPM.

## Test material
* Synthetic cadences: I-IV-V-I and i-iv-V-i, all 24 keys (unit test).
* Synthetic progressions: 24 keys x {I-V-vi-IV, I-IV-V-I, vi-IV-I-V, I-vi-ii-V} major and {i-VI-III-VII, i-iv-V-i, i-VII-VI-VII, i-iv-i-V} minor; random inversions, ±30 cent detune, drums at 70-170 BPM, 16 s each.
* GuitarSet (Xi et al., ISMIR 2018), audio_mono-mic: 360 excerpts, CC BY 4.0, Zenodo record 3371780; key_mode and tempo taken from the JAMS. Players 00-02 = dev (used for choices), 03-05 = test (held out).

## Key results (exact key %)
Rows give profile / chroma compression; the last row is shipped. Columns: Synthetic, Comp dev, Comp test, Solo dev, Solo test.

MIREX weighted scores (mir_eval `key.weighted_score` default) were re-computed by the independent verifier for the shipped row only: synthetic 0.732, GuitarSet comp test 0.670. The other rows' MIREX values from the first run used an inflated descending-fifth score and are omitted; the exact-key percentages below were reproduced and are unaffected.
* Krumhansl-Kessler / lin: 67.7 %, 58.9 %, 47.8 %, 35.6 %, 31.1 %
* Krumhansl-Kessler / log10: 66.7 %, 64.4 %, 48.9 %, 27.8 %, 27.8 %
* Temperley / lin: 44.8 %, 66.7 %, 57.8 %, 40.0 %, 38.9 %
* Temperley / log10: 59.4 %, 71.1 %, 58.9 %, 41.1 %, 40.0 %
* Albrecht-Shanahan / lin: 62.5 %, 58.9 %, 52.2 %, 46.7 %, 34.4 %
* Albrecht-Shanahan / log10 (SHIPPED): 66.7 %, 80.0 %, 58.9 %, 42.2 %, 33.3 %

n = 96 synthetic and 90 per GuitarSet cell. The dev-to-test drop (80 % to 59 %, same progressions played by other guitarists) shows the method's variance; the alternatives drop the same way.

Shipped settings by progression (exact key, 12 keys each): major I-V-vi-IV 91.7 %, I-IV-V-I 100 %, vi-IV-I-V 66.7 %, I-vi-ii-V 50.0 %; minor i-VI-III-VII 16.7 %, i-iv-V-i 100 %, i-VII-VI-VII 8.3 %, i-iv-i-V 100 %. The natural-minor progressions without a leading tone fail as relative major (MIREX 0.3).

Confidence: mean 0.91 when right vs 0.77 when wrong (synthetic); 0.88 vs 0.73 (comp test). Weakly informative.

## Tempo results (within 4 % / allowing octave errors)
* Synthetic: 60.4 % / 81.2 %
* Comp: dev 55.6 % / 58.9 %, test 67.8 % / 75.6 %
* Solo: dev 70.0 % / 78.9 %, test 60.0 % / 68.9 %
* Unit-test click tracks at 80/100/128/150 BPM: 4/4 (allowing x2, x1/2)

Mean instead of median onset aggregation raised synthetic from 41.7 % to 60.4 % and GuitarSet comp (every other excerpt) from 64 % to 70 %. std_bpm=2 and beat_track were not better.

## Not measured
Full-band songs; songs that change key (one key is returned per song). The owner's songs in test-songs/ will be the first real check.


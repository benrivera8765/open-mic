// SPDX-License-Identifier: GPL-3.0-or-later
//! Unit tests of the live pitch tracker on generated voices (WORK_ORDER-2 4.4 table and
//! section 9 row "Tracker"). The full sweep with every number is the `tracker_bench`
//! example; these tests check the same properties on enough cases to catch a regression
//! and run in a few seconds in a debug build.

use super::testvoice::{self as tv, Voice, Vowel};
use super::*;

const RATES: [u32; 4] = [16_000, 44_100, 48_000, 96_000];
/// MIDI numbers of C2 and C6.
const C2: i32 = 36;
const C6: i32 = 84;

fn track_with(rate: u32, cfg: TrackerConfig, x: &[f32]) -> Vec<PitchEvent> {
    let mut t = Tracker::with_config(rate, cfg);
    let mut out = Vec::new();
    // Odd block size: events must not depend on how the input is cut.
    for block in x.chunks(rate as usize / 100 + 13) {
        t.process(block, |e| out.push(e));
    }
    out
}

fn track(rate: u32, x: &[f32]) -> Vec<PitchEvent> {
    track_with(rate, TrackerConfig::default(), x)
}

fn secs(e: &PitchEvent, rate: u32) -> f64 {
    e.time as f64 / f64::from(rate)
}

/// Signed errors in cents of the events described in [t0, t1] against `truth`, and the
/// number of events in that span that gave no pitch.
fn errors_in(
    ev: &[PitchEvent],
    rate: u32,
    t0: f64,
    t1: f64,
    truth: impl Fn(f64) -> f64,
) -> (Vec<f64>, usize) {
    let mut errs = Vec::new();
    let mut unvoiced = 0;
    for e in ev.iter().filter(|e| (t0..=t1).contains(&secs(e, rate))) {
        match e.hz {
            Some(hz) => errs.push(tv::cents(f64::from(hz), truth(secs(e, rate)))),
            None => unvoiced += 1,
        }
    }
    (errs, unvoiced)
}

fn quantile(v: &[f64], q: f64) -> f64 {
    let mut s: Vec<f64> = v.iter().map(|x| x.abs()).collect();
    s.sort_by(|a, b| a.partial_cmp(b).unwrap());
    s[((s.len() - 1) as f64 * q).round() as usize]
}

fn octave_errors(errs: &[f64]) -> usize {
    errs.iter()
        .filter(|c| {
            let k = (*c / 1200.0).round();
            k != 0.0 && (*c - 1200.0 * k).abs() < 100.0
        })
        .count()
}

#[test]
fn geometry_is_sized_in_time() {
    for (rate, win, hop) in [
        (16_000, 683, 160),
        (44_100, 1882, 441),
        (48_000, 2048, 480),
        (96_000, 4096, 960),
    ] {
        let t = Tracker::new(rate);
        assert_eq!((t.window_len(), t.hop_len()), (win, hop), "{rate} Hz");
        assert_eq!(t.describe_delay(), (win / 2 + hop) as u64);
    }
}

#[test]
fn one_event_per_hop_with_consistent_times() {
    let rate = 48_000;
    let x = tv::note(rate, Voice::default(), 220.0, 0.1, 0.5, 0.1);
    let ev = track(rate, &x);
    assert_eq!(ev.len(), x.len() / 480);
    let t = Tracker::new(rate);
    for (k, e) in ev.iter().enumerate() {
        assert_eq!(e.ready_at, 480 * (k as u64 + 1));
        assert_eq!(e.time, e.ready_at as i64 - t.describe_delay() as i64);
    }
}

/// WO2 4.4: within 10 cents on steady tones from C2 to C6, no octave jumps. Every semitone,
/// with the vowel cycling ah / ee / oo, at 48 kHz.
#[test]
fn steady_notes_every_semitone_c2_to_c6() {
    let rate = 48_000;
    for note in C2..=C6 {
        let hz = tv::midi_hz(f64::from(note));
        let vowel = Vowel::ALL[(note % 3) as usize];
        let x = tv::note(
            rate,
            Voice {
                vowel,
                ..Voice::default()
            },
            hz,
            0.15,
            0.45,
            0.1,
        );
        let (errs, unvoiced) = errors_in(&track(rate, &x), rate, 0.25, 0.55, |_| hz);
        let name = tv::note_name(note);
        assert_eq!(unvoiced, 0, "{name} {vowel:?}: gaps in a held note");
        assert_eq!(octave_errors(&errs), 0, "{name} {vowel:?}: octave error");
        assert!(
            quantile(&errs, 1.0) <= 5.0,
            "{name} {vowel:?}: max error {:.2} cents",
            quantile(&errs, 1.0)
        );
    }
}

/// The same at 16, 44.1 and 96 kHz (vowel ah, the hardest for octave errors on low notes:
/// its first formant makes harmonics near 730 Hz louder than the fundamental).
#[test]
fn steady_notes_at_other_rates() {
    for rate in [16_000, 44_100, 96_000] {
        for note in (C2..=C6).step_by(2) {
            let hz = tv::midi_hz(f64::from(note));
            let x = tv::note(rate, Voice::default(), hz, 0.15, 0.4, 0.1);
            let (errs, unvoiced) = errors_in(&track(rate, &x), rate, 0.25, 0.5, |_| hz);
            let name = tv::note_name(note);
            assert_eq!(unvoiced, 0, "{rate} Hz {name}: gaps");
            assert_eq!(octave_errors(&errs), 0, "{rate} Hz {name}: octave error");
            assert!(
                quantile(&errs, 1.0) <= 5.0,
                "{rate} Hz {name}: max error {:.2}",
                quantile(&errs, 1.0)
            );
        }
    }
}

/// Vibrato of +-50 cents at 5 to 6 Hz is followed: errors are measured against the true
/// instantaneous pitch at each event's described time.
#[test]
fn vibrato_is_followed() {
    for rate in [16_000, 48_000] {
        for (note, vib) in [(40, 5.0), (57, 5.5), (69, 6.0), (81, 5.5)] {
            let hz = tv::midi_hz(f64::from(note));
            let f = |t: f64| tv::vibrato_hz(hz, 50.0, vib, t, 0.2);
            let x = tv::sing(rate, 1.4, Voice::default(), |t| {
                (0.2..1.2).contains(&t).then(|| f(t))
            });
            let (errs, unvoiced) = errors_in(&track(rate, &x), rate, 0.35, 1.15, f);
            let name = tv::note_name(note);
            assert_eq!(unvoiced, 0, "{rate} Hz {name}: vibrato breaks the note");
            assert_eq!(octave_errors(&errs), 0);
            assert!(
                quantile(&errs, 0.5) <= 6.0,
                "{rate} {name}: median {:.2}",
                quantile(&errs, 0.5)
            );
            assert!(
                quantile(&errs, 1.0) <= 15.0,
                "{rate} {name}: max {:.2}",
                quantile(&errs, 1.0)
            );
        }
    }
}

/// A glide of an octave in half a second stays on the line (the window and the median
/// smooth it a little).
#[test]
fn slides_are_followed() {
    for rate in [16_000, 48_000] {
        for (a, b) in [(48.0, 60.0), (69.0, 57.0)] {
            let (ha, hb) = (tv::midi_hz(a), tv::midi_hz(b));
            let f = move |t: f64| ha * (hb / ha).powf(((t - 0.5) / 0.5).clamp(0.0, 1.0));
            let x = tv::sing(rate, 1.5, Voice::default(), |t| {
                (0.2..1.3).contains(&t).then(|| f(t))
            });
            let (errs, unvoiced) = errors_in(&track(rate, &x), rate, 0.3, 1.25, f);
            assert_eq!(unvoiced, 0, "{rate} Hz {a} -> {b}: the slide breaks up");
            assert!(
                errs.iter().all(|c| c.abs() < 50.0),
                "{rate} Hz: a slide event is off the note"
            );
            assert!(
                quantile(&errs, 0.95) <= 15.0,
                "{rate} Hz: p95 {:.2}",
                quantile(&errs, 0.95)
            );
        }
    }
}

/// WO2 4.4: silence, breaths, consonants and room noise give "no pitch", and handling
/// thumps and popped consonants from a handheld microphone never become notes (4.10).
#[test]
fn not_singing_gives_no_pitch() {
    for rate in [16_000, 48_000] {
        let mut thumps = Vec::new();
        for hz in [40.0, 60.0, 80.0, 100.0, 120.0, 150.0, 200.0] {
            for peak in [-1.0, -12.0] {
                thumps.extend(tv::thump(rate, 0.4, 0.1, hz, peak));
            }
        }
        let mut pops = Vec::new();
        for (k, peak) in [-1.0, -6.0, -12.0].into_iter().enumerate() {
            pops.extend(tv::plosive(rate, 0.4, 0.1, peak, k as u64));
        }
        let cases: Vec<(&str, Vec<f32>)> = vec![
            ("silence", tv::silence(rate, 2.0)),
            ("breath", tv::breath(rate, 4.0, -25.0, 3)),
            ("loud breath", tv::breath(rate, 4.0, -12.0, 4)),
            ("speech-like noise", tv::speech_noise(rate, 5.0, -20.0, 5)),
            ("fricatives", tv::fricatives(rate, 3.0, -20.0, 6)),
            (
                "room noise with 50 Hz hum",
                tv::room_noise(rate, 3.0, -50.0, Some((50.0, -62.0)), 8),
            ),
            (
                "room noise with 60 Hz hum",
                tv::room_noise(rate, 3.0, -50.0, Some((60.0, -62.0)), 9),
            ),
            (
                "loud room noise",
                tv::room_noise(rate, 3.0, -35.0, None, 10),
            ),
            ("handling thumps", thumps),
            ("popped plosives", pops),
        ];
        for (name, x) in cases {
            let ev = track(rate, &x);
            let voiced: Vec<_> = ev.iter().filter_map(|e| e.hz).collect();
            assert!(
                voiced.is_empty(),
                "{rate} Hz {name}: {} pitched events, e.g. {:?}",
                voiced.len(),
                voiced.first()
            );
        }
    }
}

/// Minimum duration in the range where handling thumps and pops live: a loud, perfectly
/// periodic burst below 160 Hz held for 25 ms (45 ms with its ramps) gives no event, while
/// the same pitch held for 80 ms (a quick sung note) does. (Higher bursts show sooner,
/// because a window fills with their periods faster: see the "bursts" table in
/// docs/benchmarks/practice-tracker.md.)
#[test]
fn short_low_bursts_are_not_notes() {
    for rate in [16_000, 48_000] {
        for hz in [65.4, 82.4, 110.0, 155.0] {
            for p in 0..4 {
                let lead = 0.2 + (p * rate as usize / 400) as f64 / f64::from(rate);
                let loud = Voice {
                    level_dbfs: -6.0,
                    ..Voice::default()
                };
                let x = tv::note(rate, loud, hz, lead, 0.025, 0.3);
                assert!(
                    track(rate, &x).iter().all(|e| e.hz.is_none()),
                    "{rate} Hz: {hz} Hz burst became a note"
                );
                let x = tv::note(rate, loud, hz, lead, 0.08, 0.3);
                assert!(
                    track(rate, &x).iter().any(|e| e.hz.is_some()),
                    "{rate} Hz: 80 ms {hz} Hz note missed"
                );
            }
        }
    }
}

/// WO2 4.4: aim for 50 ms from the sound to the marker, never more than 100 ms. Measured
/// from the first sample of the note to the `ready_at` of the first event within 50 cents,
/// over onsets at five positions within a hop.
#[test]
fn onset_delay_within_budget() {
    let rate = 48_000;
    let mut delays = Vec::new();
    let mut releases = Vec::new();
    for note in (C2..=C6).step_by(3) {
        let hz = tv::midi_hz(f64::from(note));
        for p in 0..5 {
            let lead = 0.2 + (p * 96) as f64 / f64::from(rate);
            let x = tv::note(rate, Voice::default(), hz, lead, 0.3, 0.2);
            let ev = track(rate, &x);
            let onset = (lead * f64::from(rate)).round() as u64;
            let end = ((lead + 0.3) * f64::from(rate)).round() as u64;
            let first = ev
                .iter()
                .find(|e| {
                    e.ready_at > onset
                        && e.hz
                            .is_some_and(|h| tv::cents(f64::from(h), hz).abs() <= 50.0)
                })
                .unwrap_or_else(|| panic!("{} never found", tv::note_name(note)));
            assert!(
                ev.iter()
                    .filter(|e| e.ready_at <= onset)
                    .all(|e| e.hz.is_none()),
                "pitch before the onset"
            );
            delays.push((first.ready_at - onset) as f64 / f64::from(rate));
            let gone = ev
                .iter()
                .find(|e| e.ready_at > end && e.hz.is_none())
                .unwrap();
            releases.push((gone.ready_at - end) as f64 / f64::from(rate));
        }
    }
    let max = quantile(&delays, 1.0);
    let median = quantile(&delays, 0.5);
    assert!(max <= 0.100, "onset delay max {max:.3} s");
    assert!(median <= 0.050, "onset delay median {median:.3} s");
    assert!(
        quantile(&releases, 1.0) <= 0.100,
        "release delay max {:.3}",
        quantile(&releases, 1.0)
    );
}

/// A test melody: notes across the range, vibrato, slides and gaps.
fn melody(rate: u32, seconds: f64) -> Vec<f32> {
    let f = |t: f64| -> Option<f64> {
        let k = (t / 0.8) as i32;
        let p = t - f64::from(k) * 0.8;
        if p >= 0.7 {
            return None;
        }
        let hz = tv::midi_hz(f64::from(40 + (k * 7) % 41));
        Some(match k % 3 {
            0 => hz,
            1 => tv::vibrato_hz(hz, 50.0, 5.5, t, 0.0),
            _ => hz * 2f64.powf((p / 0.7) * 5.0 / 12.0),
        })
    };
    tv::sing(rate, seconds, Voice::default(), f)
}

/// WO2 trap 14: 16, 44.1, 48 and 96 kHz give the same answer. Event k describes the same
/// moment at every rate; compared with 48 kHz.
#[test]
fn same_answer_at_every_rate() {
    let reference = track(48_000, &melody(48_000, 8.0));
    for rate in RATES {
        let ev = track(rate, &melody(rate, 8.0));
        assert_eq!(ev.len(), reference.len());
        let mut diffs = Vec::new();
        let mut voicing = 0;
        for (a, b) in ev.iter().zip(&reference) {
            // Same moment to within half a sample at 16 kHz (the window centre is rounded
            // to a whole sample at each rate).
            assert!(
                (secs(a, rate) - secs(b, 48_000)).abs() < 0.5 / 16_000.0,
                "event times differ"
            );
            match (a.hz, b.hz) {
                (Some(x), Some(y)) => diffs.push(tv::cents(f64::from(x), f64::from(y))),
                (None, None) => {}
                _ => voicing += 1,
            }
        }
        assert!(
            voicing * 100 <= ev.len(),
            "{rate} Hz: voicing differs on {voicing} events"
        );
        assert!(
            quantile(&diffs, 0.95) <= 1.0,
            "{rate} Hz: p95 {:.2}",
            quantile(&diffs, 0.95)
        );
        assert!(
            quantile(&diffs, 1.0) <= 15.0,
            "{rate} Hz: max {:.2}",
            quantile(&diffs, 1.0)
        );
    }
}

/// WO2 trap 14 and 4.4 "choosing the channel": the voice on one channel of an interleaved
/// stream is tracked exactly as if it were alone, whatever is on the other channels.
#[test]
fn one_channel_of_an_interleaved_stream() {
    let rate = 44_100;
    let voice = melody(rate, 3.0);
    let noise = tv::speech_noise(rate, 3.0, -10.0, 9);
    let quiet = tv::silence(rate, 3.0);
    let alone = track(rate, &voice);
    for (layout, channel) in [
        (tv::interleave(&[&voice, &quiet]), 0),
        (tv::interleave(&[&noise, &voice]), 1),
        (tv::interleave(&[&quiet, &noise, &voice, &noise]), 2),
    ] {
        let channels = layout.len() / voice.len();
        let mut t = Tracker::new(rate);
        let mut ev = Vec::new();
        for block in layout.chunks(channels * 512) {
            t.process_interleaved(block, channels, channel, |e| ev.push(e));
        }
        assert_eq!(
            ev, alone,
            "channel {channel} of {channels} is not the voice alone"
        );
    }
    // And the noise channel alone gives no pitch.
    let mut t = Tracker::new(rate);
    let layout = tv::interleave(&[&voice, &noise]);
    let mut voiced = 0;
    t.process_interleaved(&layout, 2, 1, |e| voiced += usize::from(e.hz.is_some()));
    assert_eq!(voiced, 0);
}

/// Cutting the input differently (sample by sample, one block, random sizes) gives
/// bit-identical events; reset() makes a used tracker equal to a new one.
#[test]
fn block_size_and_reset_do_not_change_the_answer() {
    let rate = 16_000;
    let x = melody(rate, 2.0);
    let whole = {
        let mut t = Tracker::new(rate);
        let mut v = Vec::new();
        t.process(&x, |e| v.push(e));
        v
    };
    let mut t = Tracker::new(rate);
    let mut by_sample = Vec::new();
    for s in x.chunks(1) {
        t.process(s, |e| by_sample.push(e));
    }
    assert_eq!(by_sample, whole);
    t.reset();
    assert_eq!(t.consumed(), 0);
    let mut rng = tv::Rng::new(5);
    let mut random = Vec::new();
    let mut rest = &x[..];
    while !rest.is_empty() {
        let n = ((rng.unit() * 1500.0) as usize).min(rest.len());
        t.process(&rest[..n], |e| random.push(e));
        rest = &rest[n..];
    }
    assert_eq!(random, whole);
}

/// A broken driver's NaN or infinite samples count as silence and do not poison the
/// tracker: the next note is found normally.
#[test]
fn non_finite_input_is_silence() {
    let rate = 48_000;
    let mut x = vec![f32::NAN; 4800];
    x.extend(vec![f32::INFINITY; 480]);
    x.extend(tv::note(rate, Voice::default(), 330.0, 0.1, 0.4, 0.0));
    let ev = track(rate, &x);
    assert!(
        ev.iter()
            .all(|e| e.clarity.is_finite() && e.level_dbfs.is_finite())
    );
    let (errs, unvoiced) = errors_in(&ev, rate, 0.3, 0.6, |_| 330.0);
    assert_eq!(unvoiced, 0);
    assert!(quantile(&errs, 1.0) < 5.0);
}

/// The level gate: the same voice far below the gate gives no pitch; at a quiet singing
/// level it does.
#[test]
fn level_gate() {
    let rate = 48_000;
    let quiet = tv::note(
        rate,
        Voice {
            level_dbfs: -70.0,
            ..Voice::default()
        },
        220.0,
        0.1,
        0.5,
        0.1,
    );
    assert!(track(rate, &quiet).iter().all(|e| e.hz.is_none()));
    let soft = tv::note(
        rate,
        Voice {
            level_dbfs: -45.0,
            ..Voice::default()
        },
        220.0,
        0.1,
        0.5,
        0.1,
    );
    let (_, unvoiced) = errors_in(&track(rate, &soft), rate, 0.2, 0.55, |_| 220.0);
    assert_eq!(unvoiced, 0);
}

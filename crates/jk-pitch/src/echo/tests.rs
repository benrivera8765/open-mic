// SPDX-License-Identifier: GPL-3.0-or-later
//! Unit tests of the echo canceller on generated signals (generated music from
//! [`super::testpath::synth_music`], the synthetic singer from `tracker::testvoice`). The
//! benchmark with Creative Commons backings is `examples/echo_eval.rs`.

#![allow(clippy::needless_range_loop)] // index loops over aligned signals read plainly

use super::testpath::{self, ReturnPath};
use super::*;
use crate::tracker::testvoice::{self as tv, Voice};

const RATE: u32 = 48_000;

/// A melody for the singer, around A3: phrases of four notes of 0.6 s with 0.15 s breaths,
/// then a rest of 1.2 s (4.2 s per phrase).
fn melody(t: f64) -> Option<f64> {
    let notes = [57.0, 59.0, 60.0, 62.0, 64.0, 62.0, 60.0, 59.0];
    let p = (t / 4.2) as usize;
    let tp = t - p as f64 * 4.2;
    let k = (tp / 0.75) as usize;
    let within = tp - k as f64 * 0.75;
    (k < 4 && within < 0.6).then(|| tv::midi_hz(notes[(4 * p + k) % notes.len()]))
}

fn singer(seconds: f64, level_dbfs: f64) -> Vec<f32> {
    tv::sing(
        RATE,
        seconds,
        Voice {
            level_dbfs,
            ..Voice::default()
        },
        melody,
    )
}

/// Runs `reference` (stereo) and `input` through a canceller in 480-sample blocks; returns
/// the output and the report after every block.
fn run(ec: &mut EchoCanceller, reference: &[f32], input: &[f32]) -> (Vec<f32>, Vec<EchoReport>) {
    let blk = 480;
    let mut out = vec![0.0f32; input.len()];
    let mut reports = Vec::new();
    let mut i = 0;
    while i < input.len() {
        let n = blk.min(input.len() - i);
        ec.process(
            &reference[2 * i..2 * (i + n)],
            &input[i..i + n],
            &mut out[i..i + n],
        );
        reports.push(ec.report());
        i += n;
    }
    (out, reports)
}

/// First block time (s) at which `pred` holds and keeps holding to the end.
fn settled_at(reports: &[EchoReport], pred: impl Fn(&EchoReport) -> bool) -> Option<f64> {
    let mut first = None;
    for (k, r) in reports.iter().enumerate() {
        if pred(r) {
            first.get_or_insert(k as f64 * 0.01);
        } else {
            first = None;
        }
    }
    first
}

/// Echo left in the output relative to the echo in the input, dB, over samples `from..`:
/// the output minus the (delayed) singer is the residual echo.
fn erle_db(
    ec: &EchoCanceller,
    echo: &[f32],
    voice: Option<&[f32]>,
    out: &[f32],
    from: usize,
) -> f64 {
    let lat = ec.latency();
    let mut pe = 0.0f64;
    let mut pr = 0.0f64;
    for j in from + lat..out.len() {
        let i = j - lat;
        let v = voice.map_or(0.0, |v| f64::from(v[i]));
        let r = f64::from(out[j]) - v;
        pe += f64::from(echo[i]).powi(2);
        pr += r * r;
    }
    10.0 * (pe / pr.max(1e-30)).log10()
}

/// ERLE (dB) over consecutive 2 s windows, for diagnostics.
fn erle_trace(ec: &EchoCanceller, echo: &[f32], voice: Option<&[f32]>, out: &[f32]) -> Vec<f64> {
    let lat = ec.latency();
    let w = 2 * 48_000;
    let mut v = Vec::new();
    let mut s = 0;
    while s + w + lat <= out.len() {
        let (mut pe, mut pr) = (0.0f64, 0.0f64);
        for j in s + lat..s + lat + w {
            let i = j - lat;
            let vv = voice.map_or(0.0, |v| f64::from(v[i]));
            pe += f64::from(echo[i]).powi(2);
            pr += (f64::from(out[j]) - vv).powi(2);
        }
        v.push((10.0 * (pe / pr.max(1e-30)).log10() * 10.0).round() / 10.0);
        s += w;
    }
    v
}

#[test]
fn absent_when_nothing_comes_back() {
    let music = testpath::synth_music(RATE, 8.0, -16.0, None, 7);
    let voice = singer(8.0, -20.0);
    let mut ec = EchoCanceller::new(RATE);
    let (out, reps) = run(&mut ec, &music, &voice);
    let r = ec.report();
    println!("absent: {r:?}");
    assert_eq!(r.presence, Presence::Absent);
    assert!(
        reps.iter().all(|r| r.presence != Presence::Present),
        "never present"
    );
    let t = settled_at(&reps, |r| r.presence == Presence::Absent).unwrap();
    assert!(t <= 4.0, "absent after {t} s");
    assert_eq!(r.quality, RemovalQuality::NotNeeded);
    // Untouched (delayed) input.
    let lat = ec.latency();
    for j in lat..out.len() {
        assert_eq!(out[j], voice[j - lat]);
    }
    assert_eq!(ec.trust_window(6 * 48_000, 7 * 48_000), Trust::Clean);
}

#[test]
fn usb_return_found_measured_and_removed() {
    let secs = 14.0;
    let music = testpath::synth_music(RATE, secs, -16.0, None, 11);
    let path = ReturnPath::usb(-6.0, 0.0123);
    let (echo, _) = testpath::apply(&path, &music, 2, RATE);
    let mut ec = EchoCanceller::new(RATE);
    let (out, reps) = run(&mut ec, &music, &echo);
    let r = ec.report();
    println!("usb: {r:?}");
    let t = settled_at(&reps, |r| r.presence == Presence::Present).unwrap();
    assert!(t <= 2.0, "present after {t} s");
    let d = r.delay_ms.unwrap();
    assert!((d - 12.3).abs() < 0.05, "delay {d} ms");
    let lvl = r.level_db.unwrap();
    assert!((lvl + 6.0).abs() < 1.0, "level {lvl} dB");
    assert_eq!(r.stability, DelayStability::Stable);
    let erle = erle_db(&ec, &echo, None, &out, 6 * 48_000);
    println!("usb ERLE {erle:.1} dB, estimate {:?}", r.suppression_db);
    println!("usb trace {:?}", erle_trace(&ec, &echo, None, &out));
    assert!(erle > 30.0, "ERLE {erle} dB");
    // The reported suppression is a conservative 4 s mean: it trails a filter that is still
    // improving, never runs ahead of it.
    let est = f64::from(r.suppression_db.unwrap());
    assert!(est <= erle + 3.0, "estimate {est} dB vs {erle} dB");
    assert!(
        matches!(r.quality, RemovalQuality::Good | RemovalQuality::Marginal),
        "{:?}",
        r.quality
    );
}

#[test]
fn cable_return_with_drift_is_tracked() {
    let secs = 30.0;
    let music = testpath::synth_music(RATE, secs, -16.0, None, 13);
    let path = ReturnPath::cable(-3.0, 0.0071, 100.0);
    let (echo, delays) = testpath::apply(&path, &music, 2, RATE);
    let mut ec = EchoCanceller::new(RATE);
    let (out, _reps) = run(&mut ec, &music, &echo);
    let r = ec.report();
    println!("cable: {r:?}");
    let truth_ms = delays[delays.len() - 1] * 1e3;
    let d = r.delay_ms.unwrap();
    assert!((d - truth_ms).abs() < 0.1, "delay {d} vs {truth_ms} ms");
    let drift = r.drift_ppm.unwrap();
    assert!((drift - 100.0).abs() < 15.0, "drift {drift} ppm");
    assert_eq!(r.stability, DelayStability::Stable);
    let erle = erle_db(&ec, &echo, None, &out, 15 * 48_000);
    println!("cable ERLE {erle:.1} dB, estimate {:?}", r.suppression_db);
    println!("cable trace {:?}", erle_trace(&ec, &echo, None, &out));
    assert!(erle > 25.0, "ERLE {erle} dB");
}

#[test]
fn bluetooth_return_is_flagged_unusable() {
    let secs = 30.0;
    let music = testpath::synth_music(RATE, secs, -16.0, None, 17);
    let path = ReturnPath::bluetooth(-6.0, 0.180);
    let (echo, _delays) = testpath::apply(&path, &music, 2, RATE);
    let mut ec = EchoCanceller::new(RATE);
    let (_out, reps) = run(&mut ec, &music, &echo);
    let r = ec.report();
    println!("bluetooth: {r:?}");
    let t = settled_at(&reps, |r| r.presence == Presence::Present).unwrap();
    println!("bluetooth present after {t} s");
    // A wandering delay smears the correlation, so the detection is a candidate that must
    // stand out at its (moving) lag for about 3 s before it is believed (the quality round's
    // guard against a singer holding a note in tune with the music): found after 0.98-1.3 s
    // before, 3-8 s now (docs/benchmarks/practice-echo.md section 13).
    assert!(t <= 8.0, "present after {t} s");
    assert_eq!(r.stability, DelayStability::Wandering);
    assert_eq!(r.quality, RemovalQuality::Unusable);
}

#[test]
fn singer_over_returned_music_is_trusted_and_kept() {
    // A cable return 10 dB under the music's level (6 dB under the singer), drifting.
    let secs = 16.0;
    let music = testpath::synth_music(RATE, secs, -16.0, None, 19);
    let path = ReturnPath::cable(-10.0, 0.0050, -60.0);
    let (echo, _) = testpath::apply(&path, &music, 2, RATE);
    let voice = singer(secs, -20.0);
    let input: Vec<f32> = echo.iter().zip(&voice).map(|(a, b)| a + b).collect();
    let mut ec = EchoCanceller::new(RATE);
    let (out, _reps) = run(&mut ec, &music, &input);
    let r = ec.report();
    println!("double talk: {r:?}");
    assert_eq!(r.presence, Presence::Present);
    assert_eq!(r.stability, DelayStability::Stable);
    let erle = erle_db(&ec, &echo, Some(&voice), &out, 8 * 48_000);
    println!("double-talk ERLE {erle:.1} dB");
    println!("dt trace {:?}", erle_trace(&ec, &echo, Some(&voice), &out));
    // While somebody sings the output filter learns only in the breaths, so suppression
    // grows slowly (18-19 dB after 16 s on this generated music; 17-25 dB on real songs in
    // docs/benchmarks/practice-echo.md). The left-overs are then 24 dB or more under the
    // singer.
    assert!(erle > 15.0, "ERLE {erle} dB");
    // The newest 2.5 s (the trust history covers about 2.7 s), every 50 ms: steady
    // singing is the singer, the rest between phrases is left-over music.
    let last = (secs * 48_000.0) as i64 - ec.latency() as i64;
    let (mut sing, mut sing_n, mut music_ok, mut music_n) = (0, 0, 0, 0);
    let mut t = last as f64 / 48_000.0 - 2.5;
    while t < last as f64 / 48_000.0 - 0.05 {
        let w = ec.trust_window((t * 48_000.0) as i64, ((t + 0.02) * 48_000.0) as i64);
        let steady = [t - 0.06, t, t + 0.08].iter().all(|&u| melody(u).is_some());
        let rest = [t - 0.1, t, t + 0.1].iter().all(|&u| melody(u).is_none());
        if steady {
            sing_n += 1;
            sing += i32::from(w == Trust::Singer);
        } else if rest {
            music_n += 1;
            music_ok += i32::from(w == Trust::Music);
        }
        t += 0.05;
    }
    println!("singer {sing}/{sing_n}, rest as music {music_ok}/{music_n}");
    assert!(
        sing_n >= 10 && sing * 10 >= sing_n * 9,
        "singer trusted {sing}/{sing_n}"
    );
    assert!(
        music_n >= 5 && music_ok == music_n,
        "rest {music_ok}/{music_n}"
    );
}

/// The same return found, measured and removed at the other rates an input may run at.
#[test]
fn other_rates() {
    for rate in [16_000u32, 44_100, 96_000] {
        let secs = 10.0;
        let music = testpath::synth_music(rate, secs, -16.0, None, 37);
        let (echo, _) = testpath::apply(&ReturnPath::usb(-6.0, 0.0153), &music, 2, rate);
        let mut ec = EchoCanceller::new(rate);
        let blk = (rate / 100) as usize;
        let mut out = vec![0.0f32; echo.len()];
        let mut i = 0;
        while i < echo.len() {
            let n = blk.min(echo.len() - i);
            ec.process(
                &music[2 * i..2 * (i + n)],
                &echo[i..i + n],
                &mut out[i..i + n],
            );
            i += n;
        }
        let r = ec.report();
        let lat = ec.latency();
        let from = (5.0 * f64::from(rate)) as usize;
        let (mut pe, mut pr) = (0.0f64, 0.0f64);
        for j in from + lat..out.len() {
            pe += f64::from(echo[j - lat]).powi(2);
            pr += f64::from(out[j]).powi(2);
        }
        let erle = 10.0 * (pe / pr).log10();
        println!(
            "{rate} Hz: delay {:?} level {:?} ERLE {erle:.1}",
            r.delay_ms, r.level_db
        );
        assert_eq!(r.presence, Presence::Present, "{rate} Hz");
        assert!((r.delay_ms.unwrap() - 15.3).abs() < 0.05, "{rate} Hz delay");
        assert!((r.level_db.unwrap() + 6.0).abs() < 1.0, "{rate} Hz level");
        assert!(erle > 25.0, "{rate} Hz ERLE {erle}");
    }
}

#[test]
fn no_singer_with_guide_vocal_in_the_return_is_music() {
    // Trap 6: the reference carries the guide vocal (the right notes); nobody sings.
    let secs = 14.0;
    // The guide 8 dB above the backing, as a lead vocal sits in a mix: loud enough that the
    // tracker follows it in the returned input (checked below), so the test can fail.
    let music = testpath::synth_music(RATE, secs, -24.0, None, 23);
    let guide = singer(secs, -16.0);
    let (frames, _) = music.as_chunks::<2>();
    let reference: Vec<f32> = frames
        .iter()
        .zip(&guide)
        .flat_map(|(f, g)| [f[0] + g, f[1] + g])
        .collect();
    let path = ReturnPath::usb(0.0, 0.0090);
    let (echo, _) = testpath::apply(&path, &reference, 2, RATE);
    let mut ec = EchoCanceller::new(RATE);
    let (out, _) = run(&mut ec, &reference, &echo);
    let lat = ec.latency() as i64;
    let end = (secs * 48_000.0) as i64 - lat;
    let t = ec.trust_window(end - 2 * 48_000, end);
    println!("guide only: {:?}, trust {t:?}", ec.report());
    assert_eq!(t, Trust::Music);
    // The tracker on the cleaned output, last 2 s: any pitch it reports is flagged.
    let mut tr = crate::tracker::Tracker::new(RATE);
    let mut voiced_trusted = 0;
    let mut voiced = 0;
    let from = out.len() - 2 * 48_000;
    let mut events = Vec::new();
    tr.process(&out, |e| events.push(e));
    for e in events {
        let ti = e.time - lat;
        if (e.time as usize) < from || e.hz.is_none() {
            continue;
        }
        voiced += 1;
        let w = ec.trust_window(ti - 480, ti + 480);
        if matches!(w, Trust::Clean | Trust::Singer) {
            voiced_trusted += 1;
        }
    }
    println!("tracker pitch frames {voiced}, of them trusted {voiced_trusted}");
    assert_eq!(voiced_trusted, 0);
    // The gate on its own: the tracker on the input itself (no removal), aligned with the
    // trust history, must find the guide's notes (so this test can fail) and the gate must
    // hide every one of them.
    let mut raw_voiced = 0;
    let mut raw_trusted = 0;
    let from = (secs * 48_000.0) as i64 - 2 * 48_000;
    let mut tr = crate::tracker::Tracker::new(RATE);
    let mut events = Vec::new();
    tr.process(&echo, |e| events.push(e));
    for e in events {
        if e.time < from || e.hz.is_none() {
            continue;
        }
        raw_voiced += 1;
        if matches!(
            ec.trust_window(e.time - 480, e.time + 480),
            Trust::Clean | Trust::Singer
        ) {
            raw_trusted += 1;
        }
    }
    println!("tracker on the raw input: pitch frames {raw_voiced}, of them trusted {raw_trusted}");
    assert!(
        raw_voiced > 20,
        "the returned guide vocal is trackable: {raw_voiced}"
    );
    assert_eq!(raw_trusted, 0);
}

#[test]
fn silence_in_the_music_changes_nothing() {
    let secs = 16.0;
    let music = testpath::synth_music(RATE, secs, -16.0, Some((5.0, 11.0)), 29);
    let path = ReturnPath::usb(-10.0, 0.020);
    let (echo, _) = testpath::apply(&path, &music, 2, RATE);
    let mut ec = EchoCanceller::new(RATE);
    let (_out, reps) = run(&mut ec, &music, &echo);
    let gap: Vec<&EchoReport> = reps[600..1100].iter().collect();
    assert!(
        gap.iter().all(|r| r.presence == Presence::Present),
        "stays present in the break"
    );
    let r = ec.report();
    assert!((r.delay_ms.unwrap() - 20.0).abs() < 0.05);
    assert_eq!(r.relocks, 0);
}

#[test]
fn reset_forgets() {
    let music = testpath::synth_music(RATE, 4.0, -16.0, None, 31);
    let (echo, _) = testpath::apply(&ReturnPath::usb(0.0, 0.01), &music, 2, RATE);
    let mut ec = EchoCanceller::new(RATE);
    let _ = run(&mut ec, &music, &echo);
    assert_eq!(ec.report().presence, Presence::Present);
    ec.reset();
    let r = ec.report();
    assert_eq!(r.presence, Presence::Searching);
    assert!(!r.removing);
}

#[test]
fn loud_return_with_a_singer_is_gradable() {
    // The owner's likely wiring: the mixer's USB loopback returns the song as loud as the app
    // plays it, and the singer sings 6 dB under it. Removal takes the music 25 dB or more
    // under the singer, so once the singer has sung for a while the quality must let the
    // take be graded (good or marginal) for most of the time; the left-over measured during
    // the singing must not be optimistic; and the singer's steady notes must get through the
    // gate. Before the quality round, the left-over was measured on the singer's quieter
    // moments and this take came out "unusable" (not graded).
    let secs = 30.0;
    let music = testpath::synth_music(RATE, secs, -16.0, None, 29);
    let path = ReturnPath::usb(0.0, 0.012);
    let (echo, _) = testpath::apply(&path, &music, 2, RATE);
    let voice = singer(secs, -22.0);
    let input: Vec<f32> = echo.iter().zip(&voice).map(|(a, b)| a + b).collect();
    let mut ec = EchoCanceller::new(RATE);
    let (out, reps) = run(&mut ec, &music, &input);
    let r = ec.report();
    println!("loud return + singer: {r:?}");
    assert_eq!(r.presence, Presence::Present);
    assert_eq!(r.stability, DelayStability::Stable);
    let tail = &reps[reps.len() - 1500..];
    let usable = tail
        .iter()
        .filter(|r| matches!(r.quality, RemovalQuality::Good | RemovalQuality::Marginal))
        .count();
    println!("gradable {usable} of {} reports in the last 15 s", tail.len());
    assert!(usable * 10 >= tail.len() * 8, "gradable {usable}/{}", tail.len());
    // The true left-over over the last 15 s, against the reported one.
    let lat = ec.latency();
    let from = out.len() - 15 * 48_000;
    let mut pr = 0.0f64;
    for j in from..out.len() {
        let i = j - lat;
        pr += (f64::from(out[j]) - f64::from(voice[i])).powi(2);
    }
    let true_res = 10.0 * (pr / (out.len() - from) as f64).log10();
    let est = r.residual_dbfs.expect("residual measured");
    println!("left-over true {true_res:.1} dBFS, reported {est:.1} dBFS");
    assert!(f64::from(est) >= true_res - 3.0, "reported left-over {est} under the truth {true_res}");
    // Steady singing in the newest 2.5 s is let through.
    let last = (secs * 48_000.0) as i64 - ec.latency() as i64;
    let (mut sing, mut sing_n) = (0, 0);
    let mut t = last as f64 / 48_000.0 - 2.5;
    while t < last as f64 / 48_000.0 - 0.05 {
        let w = ec.trust_window((t * 48_000.0) as i64, ((t + 0.02) * 48_000.0) as i64);
        if [t - 0.06, t, t + 0.08].iter().all(|&u| melody(u).is_some()) {
            sing_n += 1;
            sing += i32::from(w == Trust::Singer);
        }
        t += 0.05;
    }
    println!("singer trusted {sing}/{sing_n}");
    assert!(sing_n >= 10 && sing * 10 >= sing_n * 9, "singer trusted {sing}/{sing_n}");
}

#[test]
fn loud_return_with_nobody_singing_shows_no_pitch() {
    // The same return with nobody singing: the tracker on the cleaned output must never be
    // trusted, now that the gate believes a smaller left-over.
    let secs = 20.0;
    let music = testpath::synth_music(RATE, secs, -16.0, None, 29);
    let path = ReturnPath::cable(0.0, 0.012, 80.0);
    let (echo, _) = testpath::apply(&path, &music, 2, RATE);
    let mut ec = EchoCanceller::new(RATE);
    let (out, _) = run(&mut ec, &music, &echo);
    let lat = ec.latency() as i64;
    let mut tr = crate::tracker::Tracker::new(RATE);
    let mut events = Vec::new();
    tr.process(&out, |e| events.push(e));
    let (mut voiced, mut trusted) = (0, 0);
    for e in events {
        let ti = e.time - lat;
        if ti < 0 || e.hz.is_none() {
            continue;
        }
        voiced += 1;
        if matches!(ec.trust_window(ti - 480, ti + 480), Trust::Clean | Trust::Singer) {
            trusted += 1;
        }
    }
    println!("nobody singing: pitch frames {voiced}, trusted {trusted}; {:?}", ec.report());
    assert_eq!(trusted, 0);
}

#[test]
fn fixed_delay_with_scattered_false_peaks_does_not_wander() {
    // A fixed delay measured every 0.128 s over 20 s, with a third of the measurements false
    // peaks anywhere within 50 ms (a singer holding a note in tune with the music, under a
    // weak return): the line is the delay and it does not wander. The same with the delay
    // jumping by 8 ms halfway (Bluetooth) wanders.
    let mut rng = tv::Rng::new(5);
    let (mut t, mut d, mut dj) = (Vec::new(), Vec::new(), Vec::new());
    for k in 0..156 {
        let tk = k as f64 * 0.128;
        t.push(tk);
        let base = 0.0123 + 0.000_002 * rng.bipolar();
        let v = if k % 3 == 1 { 0.0123 + 0.025 * rng.bipolar() } else { base };
        d.push(v);
        dj.push(if tk >= 10.0 { base + 0.008 } else { base });
    }
    let now = *t.last().unwrap();
    let f = detect::fit_history(&t, &d, now, 20.0).expect("fit");
    println!("scattered: delay {:.5} ms rms {:.4} ms n {} of {} cluster2 {}", f.delay_now_s * 1e3, f.rms_s * 1e3, f.n, f.total, f.cluster2);
    assert!((f.delay_now_s - 0.0123).abs() < 0.000_05);
    assert!(!f.wanders(0.000_25));
    let fj = detect::fit_history(&t, &dj, now, 20.0).expect("fit");
    println!("jump: rms {:.4} ms n {} of {} cluster2 {}", fj.rms_s * 1e3, fj.n, fj.total, fj.cluster2);
    assert!(fj.wanders(0.000_25));
}

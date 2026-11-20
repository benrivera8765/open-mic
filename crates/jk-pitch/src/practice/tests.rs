// SPDX-License-Identifier: GPL-3.0-or-later
//! The whole chain on generated audio: a synthetic singer (tracker::testvoice), generated
//! music (echo::testpath::synth_music) and a generated mixer return. Microphone sound is
//! never used.

use super::*;
use crate::echo::testpath::{self, ReturnPath};
use crate::score::{LineBasis, LineSpan, ScoreNote};
use crate::tracker::testvoice::{self as tv, Voice, Vowel};

/// A 16-note melody from 2 s, 0.45 s notes with 0.15 s gaps (C3..C4 region, male range).
fn guide() -> ScoreGuide {
    let pitches = [48.0, 50.0, 52.0, 53.0, 55.0, 53.0, 52.0, 50.0, 48.0, 55.0, 57.0, 55.0, 53.0, 52.0, 50.0, 48.0];
    let notes: Vec<ScoreNote> = pitches
        .iter()
        .enumerate()
        .map(|(i, &m)| {
            let start = 2.0 + 0.6 * i as f64;
            ScoreNote { start, end: start + 0.45, midi: m, pitched: true, confidence: 0.9, octave_flagged: false, glide_in: 0.0, line: None }
        })
        .collect();
    let lines: Vec<LineSpan> = ScoreGuide::phrases(&notes);
    let duration_s = notes.last().unwrap().end + 1.5;
    ScoreGuide { notes, low_confidence: 0.5, lines, basis: LineBasis::Phrases, outdated: false, duration_s, label: "test 0".into() }
}

/// The synthetic singer following the guide (`cents` off, `late` seconds late).
fn singer(g: &ScoreGuide, rate: u32, cents: f64, late: f64) -> Vec<f32> {
    let notes = g.notes.clone();
    tv::sing(rate, g.duration_s, Voice { vowel: Vowel::Ah, level_dbfs: -20.0, breath_db: None, seed: 3 }, move |t| {
        let t = t - late;
        notes.iter().find(|n| t >= n.start && t < n.end).map(|n| tv::midi_hz(n.midi + cents / 100.0))
    })
}

/// Runs the chain over `input` (interleaved, `channels`) with an optional stereo
/// reference, 10 ms blocks, the song starting at host time 1 s, and returns the result,
/// the shown pitch count and the chain.
fn run(g: &ScoreGuide, cfg: ChainConfig, input: &[f32], reference: Option<&[f32]>) -> (ScoreResult, usize, Chain) {
    let rate = cfg.input_rate;
    let ch = cfg.input_channels;
    let rc = cfg.reference_channels;
    let mut chain = Chain::new(cfg, g.clone(), KeyTimeline::constant(0), Vec::new());
    let block = (rate / 100) as usize;
    let frames = input.len() / ch;
    let host0: i64 = 1_000_000_000;
    let mut live = PracticeLive::default();
    let mut shown = 0usize;
    let mut f = 0usize;
    while f < frames {
        let n = block.min(frames - f);
        let host = host0 + (f as f64 * 1e9 / f64::from(rate)).round() as i64;
        chain.playback_anchor(ClockAnchor { host_ns: host, song_s: f as f64 / f64::from(rate), segment: 0, playing: true });
        let refb: &[f32] = match reference {
            Some(r) => &r[f * rc..(f + n) * rc],
            None => &[],
        };
        chain.process(host, &input[f * ch..(f + n) * ch], refb);
        chain.drain_live(&mut live);
        shown += live.pitch.iter().filter(|p| p.1.is_some()).count();
        f += n;
    }
    let r = chain.finish(true, &[]);
    (r, shown, chain)
}

fn no_ref(rate: u32, channels: usize, channel: usize) -> ChainConfig {
    ChainConfig { reference_channels: 0, ..ChainConfig::new(rate, channels, channel) }
}

#[test]
fn scripted_singers_through_the_chain() {
    let g = guide();
    let rate = 48_000;
    let perfect = run(&g, no_ref(rate, 1, 0), &singer(&g, rate, 0.0, 0.0), None).0;
    assert!(perfect.overall.unwrap() >= 90.0, "perfect {:?}", perfect.overall);
    assert!(perfect.tendency_cents.unwrap().abs() <= 5.0, "{:?}", perfect.tendency_cents);
    let sharp = run(&g, no_ref(rate, 1, 0), &singer(&g, rate, 70.0, 0.0), None).0;
    let o = sharp.overall.unwrap();
    assert!((45.0..=72.0).contains(&o), "70 cents sharp {o}");
    assert!((sharp.tendency_cents.unwrap() - 70.0).abs() <= 6.0, "{:?}", sharp.tendency_cents);
    let late = run(&g, no_ref(rate, 1, 0), &singer(&g, rate, 0.0, 0.1), None).0;
    assert!(late.overall.unwrap() >= perfect.overall.unwrap() - 3.0, "100 ms late {:?}", late.overall);
    let silent = run(&g, no_ref(rate, 1, 0), &tv::silence(rate, g.duration_s), None);
    assert_eq!(silent.0.overall, Some(0.0));
    assert_eq!(silent.1, 0, "no marker with nobody singing");
    // Deterministic.
    let again = run(&g, no_ref(rate, 1, 0), &singer(&g, rate, 0.0, 0.0), None).0;
    assert_eq!(again, perfect);
}

#[test]
fn voice_on_one_channel_at_any_rate() {
    let g = guide();
    let mut grades = Vec::new();
    for rate in [44_100u32, 48_000, 96_000] {
        let v = singer(&g, rate, 0.0, 0.0);
        let noise = tv::room_noise(rate, g.duration_s, -40.0, None, 7);
        let both = tv::interleave(&[&noise, &v]);
        let r = run(&g, no_ref(rate, 2, 1), &both, None).0;
        grades.push(r.overall.unwrap());
    }
    let (lo, hi) = grades.iter().fold((f64::MAX, f64::MIN), |(a, b), &x| (a.min(x), b.max(x)));
    assert!(lo >= 88.0 && hi - lo <= 4.0, "{grades:?}");
}

#[test]
fn returned_music_with_nobody_singing_gives_no_marker_and_the_lowest_grade() {
    let g = guide();
    let rate = 48_000;
    let music = testpath::synth_music(rate, g.duration_s, -20.0, None, 11);
    let (ret, _) = testpath::apply(&ReturnPath::usb(-3.0, 0.012), &music, 2, rate);
    let cfg = ChainConfig::new(rate, 1, 0);
    let (r, shown, chain) = run(&g, cfg, &ret, Some(&music));
    // The lowest grade: 0, or not graded while the canceller still calls its removal
    // unusable (it is still learning the path in a take this short).
    assert!(r.overall == Some(0.0) || (r.overall.is_none() && r.trust.iter().any(|f| f == "untrusted")), "{r:?}");
    assert_eq!(r.voiced_share, 0.0);
    assert_eq!(shown, 0, "the marker never shows");
    assert!(r.trust.iter().any(|f| f == "music_in_input"), "{:?}", r.trust);
    let m = chain.music();
    assert_eq!(m.presence, Presence::Present);
    let d = m.delay_ms.unwrap();
    assert!((d - 12.0).abs() < 1.0, "measured delay {d}");
    assert_eq!(chain.latency().source, LatencySource::ReturnedMusic);
}

#[test]
fn the_singer_is_graded_through_returned_music_with_its_delay_taken_off() {
    let g = guide();
    let rate = 48_000;
    let music = testpath::synth_music(rate, g.duration_s, -24.0, None, 5);
    let delay = 0.040;
    let (ret, _) = testpath::apply(&ReturnPath::usb(-6.0, delay), &music, 2, rate);
    // The singer hears the song `delay` late (the same path), so their voice arrives late
    // by the same amount: the measured delay puts it back in time.
    let v = singer(&g, rate, 0.0, delay);
    let input: Vec<f32> = v.iter().zip(&ret).map(|(a, b)| a + b).collect();
    let (r, _, chain) = run(&g, ChainConfig::new(rate, 1, 0), &input, Some(&music));
    let clean = run(&g, no_ref(rate, 1, 0), &singer(&g, rate, 0.0, 0.0), None).0;
    let o = r.overall.unwrap();
    assert!(o >= clean.overall.unwrap() - 8.0, "with returned music {o} vs clean {:?}", clean.overall);
    let lat = chain.latency();
    assert!((lat.applied_ms - 40.0).abs() < 1.0, "{lat:?}");
}

#[test]
fn song_time_follows_the_clock_anchors_and_declared_latency() {
    // The song started at host time 5 s at song position 10 s; the input is stamped on the
    // host clock and declares 20 ms of latency.
    let g = guide();
    let rate = 48_000;
    let cfg = ChainConfig { declared_input_latency_s: 0.020, ..no_ref(rate, 1, 0) };
    let mut chain = Chain::new(cfg, g, KeyTimeline::constant(0), Vec::new());
    for i in 0..400 {
        let host = 5_000_000_000 + i * 5_000_000;
        chain.playback_anchor(ClockAnchor { host_ns: host, song_s: 10.0 + i as f64 * 0.005, segment: 3, playing: true });
    }
    // Frame k of the input was captured 20 ms before host 5.5 s + k / rate.
    let tone = tv::note(rate, Voice::default(), 220.0, 0.0, 1.0, 0.0);
    let mut live = PracticeLive::default();
    chain.process(5_500_000_000, &tone, &[]);
    chain.drain_live(&mut live);
    assert_eq!(live.segment, 3);
    let first = live.pitch.first().unwrap().0;
    // The same tracker alone: the input sample its first value describes.
    let mut t = Tracker::new(rate);
    let mut t0 = None;
    t.process(&tone, |e| {
        t0.get_or_insert(e.time);
    });
    let expect = 10.5 - 0.020 + t0.unwrap() as f64 / f64::from(rate);
    assert!((first - expect).abs() < 0.002, "first {first} expected {expect}");
    let steps: Vec<f64> = live.pitch.windows(2).map(|w| w[1].0 - w[0].0).collect();
    assert!(steps.iter().all(|s| (s - 0.01).abs() < 1e-6), "10 ms apart");
}

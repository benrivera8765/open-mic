// SPDX-License-Identifier: GPL-3.0-or-later
//! Measurements of the live pitch tracker on generated voices (WORK_ORDER-2 section 9,
//! row "Tracker"); the numbers in docs/benchmarks/practice-tracker.md come from here.
//!
//!     cargo run --release -p jk-pitch --example tracker_bench [section ...]
//!
//! Sections: steady vibrato slides breathy noise latency rates bursts cpu clarity (default:
//! all). JK_CLARITY_ON, JK_CLARITY_OFF, JK_PEAK_K, JK_MIN_RUN and JK_MAX_STEP override the
//! settings for exploration; JK_SHOW_FALSE lists any pitched event on a not-singing signal.
//! Prints Markdown tables to stdout. Only generated signals are used.

use std::time::Instant;

#[path = "tracker_set/mod.rs"]
mod tracker_set;

use jk_pitch::tracker::testvoice::{self as tv, Voice, Vowel};
use jk_pitch::tracker::{PitchEvent, Tracker, TrackerConfig};

const RATES: [u32; 4] = [16_000, 44_100, 48_000, 96_000];
/// MIDI numbers of C2 and C6.
const LOW: i32 = 36;
const HIGH: i32 = 84;

fn track(rate: u32, cfg: &TrackerConfig, x: &[f32]) -> Vec<PitchEvent> {
    let mut t = Tracker::with_config(rate, cfg.clone());
    let mut out = Vec::with_capacity(x.len() / t.hop_len() + 1);
    // Feed in 10 ms blocks of an odd size, as a device callback might.
    for block in x.chunks(rate as usize / 100 + 7) {
        t.process(block, |e| out.push(e));
    }
    out
}

fn secs(e: &PitchEvent, rate: u32) -> f64 {
    e.time as f64 / f64::from(rate)
}

fn percentile(v: &mut [f64], p: f64) -> f64 {
    if v.is_empty() {
        return f64::NAN;
    }
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let i = ((v.len() - 1) as f64 * p).round() as usize;
    v[i]
}

fn register(note: i32) -> &'static str {
    match note {
        ..48 => "C2-B2",
        48..60 => "C3-B3",
        60..72 => "C4-B4",
        _ => "C5-C6",
    }
}

/// Errors of the voiced events whose described time lies in [t0, t1], against `truth`.
/// Returns (abs cents errors, signed errors, events in range, voiced events).
fn errors(
    ev: &[PitchEvent],
    rate: u32,
    t0: f64,
    t1: f64,
    truth: impl Fn(f64) -> f64,
) -> (Vec<f64>, Vec<f64>, usize, usize) {
    let mut abs = Vec::new();
    let mut signed = Vec::new();
    let mut n = 0;
    for e in ev.iter().filter(|e| (t0..=t1).contains(&secs(e, rate))) {
        n += 1;
        if let Some(hz) = e.hz {
            let c = tv::cents(f64::from(hz), truth(secs(e, rate)));
            abs.push(c.abs());
            signed.push(c);
        }
    }
    let v = abs.len();
    (abs, signed, n, v)
}

fn is_octave_error(c: f64) -> bool {
    let k = (c / 1200.0).round();
    k != 0.0 && (c - 1200.0 * k).abs() < 100.0
}

#[derive(Default)]
struct Agg {
    abs: Vec<f64>,
    octave: usize,
    gross: usize,
    events: usize,
    voiced: usize,
    min_cover: f64,
    min_clarity: f32,
}

impl Agg {
    fn new() -> Self {
        Self {
            min_cover: 1.0,
            min_clarity: 1.0,
            ..Self::default()
        }
    }
    fn add(&mut self, abs: Vec<f64>, signed: &[f64], n: usize, v: usize) {
        self.octave += signed.iter().filter(|c| is_octave_error(**c)).count();
        self.gross += signed.iter().filter(|c| c.abs() > 100.0).count();
        self.events += n;
        self.voiced += v;
        if n > 0 {
            self.min_cover = self.min_cover.min(v as f64 / n as f64);
        }
        self.abs.extend(abs);
    }
    fn row(&mut self) -> String {
        let n = self.abs.len();
        let med = percentile(&mut self.abs, 0.5);
        let p95 = percentile(&mut self.abs, 0.95);
        let max = percentile(&mut self.abs, 1.0);
        format!(
            "{med:.2} | {p95:.2} | {max:.2} | {} | {} | {:.1} % | {:.1} % | {n}",
            self.octave,
            self.gross,
            100.0 * self.voiced as f64 / self.events.max(1) as f64,
            100.0 * self.min_cover
        )
    }
}

fn steady(cfg: &TrackerConfig) {
    println!("\n## Steady notes C2..C6, every semitone, three vowels\n");
    println!("Each note: 0.2 s silence, 0.6 s held at -20 dBFS, 0.2 s silence. Events whose");
    println!("described time lies 0.1 s after the onset to 0.05 s before the release.\n");
    println!(
        "| rate | register | median abs err (cents) | p95 | max | octave errors | gross (>100 c) | voiced | worst note voiced | events |"
    );
    println!("|---|---|---|---|---|---|---|---|---|---|");
    for rate in RATES {
        let mut all = Agg::new();
        for reg in ["C2-B2", "C3-B3", "C4-B4", "C5-C6"] {
            let mut agg = Agg::new();
            for note in (LOW..=HIGH).filter(|n| register(*n) == reg) {
                let hz = tv::midi_hz(f64::from(note));
                for vowel in Vowel::ALL {
                    let voice = Voice {
                        vowel,
                        ..Voice::default()
                    };
                    let x = tv::note(rate, voice, hz, 0.2, 0.6, 0.2);
                    let ev = track(rate, cfg, &x);
                    let (a, s, n, v) = errors(&ev, rate, 0.3, 0.75, |_| hz);
                    for e in ev.iter().filter(|e| (0.3..=0.75).contains(&secs(e, rate))) {
                        agg.min_clarity = agg.min_clarity.min(e.clarity);
                    }
                    all.add(a.clone(), &s, n, v);
                    agg.add(a, &s, n, v);
                }
            }
            println!("| {rate} | {reg} | {} |", agg.row());
            eprintln!("  steady {rate} {reg}: min clarity {:.3}", agg.min_clarity);
        }
        println!("| {rate} | all | {} |", all.row());
    }
}

fn vibrato(cfg: &TrackerConfig) {
    println!("\n## Vibrato (+-50 cents) and its effect on accuracy\n");
    println!("1.5 s notes, vowels ah and oo, error against the true instantaneous pitch at each");
    println!("event's described time (0.15 s after onset to 0.05 s before release).\n");
    println!(
        "| rate | vibrato | register | median abs err | p95 | max | octave errors | gross | voiced | worst voiced | events |"
    );
    println!("|---|---|---|---|---|---|---|---|---|---|---|");
    for rate in RATES {
        for vib in [5.0, 5.5, 6.0] {
            for reg in ["C2-B2", "C3-B3", "C4-B4", "C5-C6"] {
                let mut agg = Agg::new();
                for note in (LOW..=HIGH).step_by(3).filter(|n| register(*n) == reg) {
                    let hz = tv::midi_hz(f64::from(note));
                    for vowel in [Vowel::Ah, Vowel::Oo] {
                        let voice = Voice {
                            vowel,
                            ..Voice::default()
                        };
                        let f = |t: f64| tv::vibrato_hz(hz, 50.0, vib, t, 0.2);
                        let x =
                            tv::sing(rate, 1.9, voice, |t| (0.2..1.7).contains(&t).then(|| f(t)));
                        let ev = track(rate, cfg, &x);
                        let (a, s, n, v) = errors(&ev, rate, 0.35, 1.65, f);
                        agg.add(a, &s, n, v);
                    }
                }
                println!("| {rate} | {vib} Hz | {reg} | {} |", agg.row());
            }
        }
    }
}

fn slides(cfg: &TrackerConfig) {
    println!("\n## Slides (glides between notes)\n");
    println!(
        "Pitch moves linearly in cents between two held notes (0.3 s each). Error against the"
    );
    println!("true pitch at each event's described time, over the whole sung part (0.1 s after");
    println!("onset to 0.05 s before release).\n");
    println!(
        "| rate | slide | speed (cents/s) | median abs err | p95 | max | octave errors | gross | voiced | worst voiced | events |"
    );
    println!("|---|---|---|---|---|---|---|---|---|---|---|");
    let cases: [(i32, i32, f64); 6] = [
        (48, 60, 0.5),  // C3 -> C4 in 0.5 s
        (60, 48, 0.5),  // C4 -> C3
        (57, 69, 0.25), // A3 -> A4 in 0.25 s
        (43, 55, 1.0),  // G2 -> G3 in 1 s
        (72, 74, 0.1),  // C5 -> D5 in 0.1 s
        (69, 81, 0.3),  // A4 -> A5
    ];
    for rate in RATES {
        for (a, b, dur) in cases {
            let (ha, hb) = (tv::midi_hz(f64::from(a)), tv::midi_hz(f64::from(b)));
            let f = move |t: f64| {
                let p = ((t - 0.5) / dur).clamp(0.0, 1.0);
                ha * (hb / ha).powf(p)
            };
            let end = 0.5 + dur + 0.3;
            let x = tv::sing(rate, end + 0.2, Voice::default(), |t| {
                (0.2..end).contains(&t).then(|| f(t))
            });
            let ev = track(rate, cfg, &x);
            let (abs, s, n, v) = errors(&ev, rate, 0.3, end - 0.05, f);
            let mut agg = Agg::new();
            agg.add(abs, &s, n, v);
            let speed = 1200.0 * (f64::from(b - a) / 12.0).abs() / dur;
            println!(
                "| {rate} | {} -> {} in {dur} s | {speed:.0} | {} |",
                tv::note_name(a),
                tv::note_name(b),
                agg.row()
            );
        }
    }
}

fn breathy(cfg: &TrackerConfig) {
    println!("\n## Breathy voice (aspiration noise through the vowel)\n");
    println!(
        "| rate | breath noise | median abs err | p95 | max | octave errors | gross | voiced | worst voiced | events |"
    );
    println!("|---|---|---|---|---|---|---|---|---|---|");
    for rate in [16_000, 48_000] {
        for db in [-20.0, -12.0, -6.0] {
            let mut agg = Agg::new();
            for note in (LOW..=HIGH).step_by(4) {
                let hz = tv::midi_hz(f64::from(note));
                let voice = Voice {
                    breath_db: Some(db),
                    seed: note as u64,
                    ..Voice::default()
                };
                let x = tv::note(rate, voice, hz, 0.2, 0.6, 0.2);
                let ev = track(rate, cfg, &x);
                let (a, s, n, v) = errors(&ev, rate, 0.3, 0.75, |_| hz);
                agg.add(a, &s, n, v);
            }
            println!("| {rate} | {db} dB | {} |", agg.row());
        }
    }
}

fn noise(cfg: &TrackerConfig) {
    println!("\n## Not singing: no pitch, no wandering\n");
    println!("Voiced events reported (must be 0), and the highest raw clarity seen (how close");
    println!(
        "the sound came to the clarity_on threshold of {}).\n",
        cfg.clarity_on
    );
    println!(
        "| signal | rate | seconds | voiced events | longest raw voiced run (frames) | highest clarity | loudest window (dBFS) |"
    );
    println!("|---|---|---|---|---|---|---|");
    for rate in RATES {
        let mut thumps = Vec::new();
        for hz in [40.0, 60.0, 80.0, 100.0, 120.0, 150.0, 200.0] {
            for peak in [-1.0, -12.0] {
                thumps.extend(tv::thump(rate, 0.5, 0.1, hz, peak));
            }
        }
        let mut pops = Vec::new();
        for (k, peak) in [-1.0, -6.0, -12.0].iter().enumerate() {
            for _ in 0..3 {
                pops.extend(tv::plosive(rate, 0.5, 0.1, *peak, k as u64 + 7));
            }
        }
        let signals: Vec<(&str, Vec<f32>)> = vec![
            ("digital silence", tv::silence(rate, 3.0)),
            ("breath, -25 dBFS", tv::breath(rate, 30.0, -25.0, 3)),
            (
                "breath, -12 dBFS (very close)",
                tv::breath(rate, 30.0, -12.0, 4),
            ),
            (
                "speech-like noise, -20 dBFS",
                tv::speech_noise(rate, 30.0, -20.0, 5),
            ),
            (
                "fricatives s/sh, -20 dBFS",
                tv::fricatives(rate, 20.0, -20.0, 6),
            ),
            (
                "room noise -50 dBFS + 50 Hz hum -62 dBFS",
                tv::room_noise(rate, 5.0, -50.0, Some((50.0, -62.0)), 8),
            ),
            (
                "room noise -50 dBFS + 60 Hz hum -62 dBFS",
                tv::room_noise(rate, 5.0, -50.0, Some((60.0, -62.0)), 9),
            ),
            (
                "room noise -35 dBFS (loud)",
                tv::room_noise(rate, 5.0, -35.0, None, 10),
            ),
            (
                "room noise -50 dBFS + 50 Hz hum -40 dBFS (above the gate)",
                tv::room_noise(rate, 5.0, -50.0, Some((50.0, -40.0)), 11),
            ),
            (
                "room noise -50 dBFS + 60 Hz hum -40 dBFS (above the gate)",
                tv::room_noise(rate, 5.0, -50.0, Some((60.0, -40.0)), 12),
            ),
            (
                "handling thumps 40..200 Hz, peak -1 / -12 dBFS (14)",
                thumps,
            ),
            ("popped plosives, peak -1 / -6 / -12 dBFS (9)", pops),
        ];
        for (name, x) in signals {
            let ev = track(rate, cfg, &x);
            let voiced = ev.iter().filter(|e| e.hz.is_some()).count();
            if std::env::var("JK_SHOW_FALSE").is_ok() {
                for e in ev.iter().filter(|e| e.hz.is_some()) {
                    eprintln!(
                        "  false: {name} {rate} t={:.2} hz={:.1} clarity={:.3}",
                        secs(e, rate),
                        e.hz.unwrap(),
                        e.clarity
                    );
                }
            }
            let clar = ev.iter().fold(0.0f32, |m, e| m.max(e.clarity));
            let lvl = ev.iter().fold(-200.0f32, |m, e| m.max(e.level_dbfs));
            let raw = track(
                rate,
                &TrackerConfig {
                    min_voiced_frames: 1,
                    ..cfg.clone()
                },
                &x,
            );
            let (mut run, mut longest) = (0, 0);
            for e in &raw {
                run = if e.hz.is_some() { run + 1 } else { 0 };
                longest = longest.max(run);
            }
            println!(
                "| {name} | {rate} | {:.1} | {voiced} | {longest} | {clar:.3} | {lvl:.1} |",
                x.len() as f64 / f64::from(rate)
            );
        }
    }
}

/// Onset delay: from the first sample of the note to the `ready_at` of the first event
/// within `tol` cents, over `phases` onset positions spread across one hop.
fn onset_delays(
    rate: u32,
    cfg: &TrackerConfig,
    note: i32,
    vowel: Vowel,
    phases: usize,
    tol: f64,
) -> (Vec<f64>, Vec<f64>) {
    let hz = tv::midi_hz(f64::from(note));
    let hop = rate as usize / 100;
    let mut on = Vec::new();
    let mut off = Vec::new();
    for p in 0..phases {
        let lead = 0.3 + (p * hop / phases) as f64 / f64::from(rate);
        let x = tv::note(
            rate,
            Voice {
                vowel,
                ..Voice::default()
            },
            hz,
            lead,
            0.5,
            0.3,
        );
        let ev = track(rate, cfg, &x);
        let onset = (lead * f64::from(rate)).round() as u64;
        let first = ev.iter().find(|e| {
            e.ready_at > onset
                && e.hz
                    .is_some_and(|h| tv::cents(f64::from(h), hz).abs() <= tol)
        });
        on.push(first.map_or(f64::INFINITY, |e| {
            (e.ready_at - onset) as f64 / f64::from(rate)
        }));
        // Release: the 10 ms ramp starts at lead + 0.5 s.
        let end = ((lead + 0.5) * f64::from(rate)).round() as u64;
        let gone = ev.iter().find(|e| e.ready_at > end && e.hz.is_none());
        off.push(gone.map_or(f64::INFINITY, |e| {
            (e.ready_at - end) as f64 / f64::from(rate)
        }));
    }
    (on, off)
}

fn latency(cfg: &TrackerConfig) {
    println!("\n## Delay from the onset of a note to the first correct event\n");
    println!(
        "Onset = first sample of a 10 ms ramp after 0.3 s of silence; 10 onset positions spread"
    );
    println!(
        "over one hop; every semitone C2..C6, vowel ah. 'Correct' = within 50 cents (the right"
    );
    println!(
        "note) or within 10 cents. 'Raw' = the same tracker with min_voiced_frames = 1 (no extra"
    );
    println!("hop for the minimum run and the median): the difference is the median's share.");
    println!("Release = from the start of the 10 ms release ramp to the first 'no pitch' event.\n");
    println!(
        "| rate | register | to 50 c: median / max (ms) | to 10 c: median / max | raw to 50 c: median / max | release: median / max |"
    );
    println!("|---|---|---|---|---|---|");
    let raw_cfg = TrackerConfig {
        min_voiced_frames: 1,
        ..cfg.clone()
    };
    for rate in RATES {
        for reg in ["C2-B2", "C3-B3", "C4-B4", "C5-C6", "all"] {
            let (mut a, mut b, mut c, mut d) = (vec![], vec![], vec![], vec![]);
            for note in (LOW..=HIGH).filter(|n| reg == "all" || register(*n) == reg) {
                let (on, off) = onset_delays(rate, cfg, note, Vowel::Ah, 10, 50.0);
                a.extend(on);
                d.extend(off);
                b.extend(onset_delays(rate, cfg, note, Vowel::Ah, 10, 10.0).0);
                c.extend(onset_delays(rate, &raw_cfg, note, Vowel::Ah, 10, 50.0).0);
            }
            let f = |v: &mut Vec<f64>| {
                format!(
                    "{:.1} / {:.1}",
                    1e3 * percentile(v, 0.5),
                    1e3 * percentile(v, 1.0)
                )
            };
            println!(
                "| {rate} | {reg} | {} | {} | {} | {} |",
                f(&mut a),
                f(&mut b),
                f(&mut c),
                f(&mut d)
            );
        }
    }

    // The window's share alone: the same tracker analysing after every sample (hop = one
    // sample, min_voiced_frames = 1), so there is no hop wait and no median frame: the
    // time from the onset until the window holds enough of the note for clarity_on.
    println!("\n### The window's share of the delay\n");
    println!("Tracker re-run with an analysis after every input sample and min_voiced_frames = 1:");
    println!("onset to the first sample whose window gives a correct (within 50 cents) voiced");
    println!("estimate. Total delay = this + the wait for the next 10 ms hop (0..10 ms) + one hop");
    println!("for the minimum run / median.\n");
    println!("| rate | register | window fill: median / max (ms) |");
    println!("|---|---|---|");
    for rate in [16_000, 48_000] {
        let fill_cfg = TrackerConfig {
            hop_s: 1.0 / f64::from(rate),
            min_voiced_frames: 1,
            ..cfg.clone()
        };
        for reg in ["C2-B2", "C3-B3", "C4-B4", "C5-C6", "all"] {
            let mut v = Vec::new();
            for note in (LOW..=HIGH).filter(|n| reg == "all" || register(*n) == reg) {
                let hz = tv::midi_hz(f64::from(note));
                let x = tv::note(rate, Voice::default(), hz, 0.1, 0.12, 0.0);
                let onset = (0.1 * f64::from(rate)).round() as u64;
                let ev = track(rate, &fill_cfg, &x);
                let first = ev.iter().find(|e| {
                    e.ready_at > onset
                        && e.hz
                            .is_some_and(|h| tv::cents(f64::from(h), hz).abs() <= 50.0)
                });
                v.push(first.map_or(f64::INFINITY, |e| {
                    (e.ready_at - onset) as f64 / f64::from(rate)
                }));
            }
            println!(
                "| {rate} | {reg} | {:.1} / {:.1} |",
                1e3 * percentile(&mut v.clone(), 0.5),
                1e3 * percentile(&mut v, 1.0)
            );
        }
    }
}

/// A test melody exercising everything: notes across the range, vibrato, slides, gaps.
fn melody(rate: u32, seconds: f64) -> Vec<f32> {
    let f = |t: f64| -> Option<f64> {
        let k = (t / 0.8) as i32; // a new phrase element every 0.8 s
        let p = t - f64::from(k) * 0.8;
        if p >= 0.7 {
            return None; // 0.1 s gap
        }
        let base = 40 + (k * 7) % 41; // E2 .. G#5, wandering
        let hz = tv::midi_hz(f64::from(base));
        Some(match k % 3 {
            0 => hz,
            1 => tv::vibrato_hz(hz, 50.0, 5.5, t, 0.0),
            _ => hz * 2f64.powf((p / 0.7) * 5.0 / 12.0),
        })
    };
    let voice = Voice {
        vowel: Vowel::Ah,
        ..Voice::default()
    };
    tv::sing(rate, seconds, voice, f)
}

fn rates(cfg: &TrackerConfig) {
    println!("\n## Same answer at every sample rate\n");
    println!(
        "A 40 s generated melody (notes E2..G#5, steady, vibrato and slides, 0.1 s gaps) at each"
    );
    println!("rate; event k describes the same moment at every rate. Compared with 48 kHz.\n");
    println!(
        "| rate | events | voicing differs | both voiced | median abs diff (cents) | p95 | max |"
    );
    println!("|---|---|---|---|---|---|---|");
    let reference = track(48_000, cfg, &melody(48_000, 40.0));
    for rate in RATES {
        let ev = track(rate, cfg, &melody(rate, 40.0));
        let n = ev.len().min(reference.len());
        let mut diffs = Vec::new();
        let mut voicing = 0;
        for (a, b) in ev[..n].iter().zip(&reference[..n]) {
            match (a.hz, b.hz) {
                (Some(x), Some(y)) => diffs.push(tv::cents(f64::from(x), f64::from(y)).abs()),
                (None, None) => {}
                _ => voicing += 1,
            }
        }
        let both = diffs.len();
        println!(
            "| {rate} | {n} | {voicing} | {both} | {:.2} | {:.2} | {:.2} |",
            percentile(&mut diffs, 0.5),
            percentile(&mut diffs, 0.95),
            percentile(&mut diffs, 1.0)
        );
    }
}

fn cpu(cfg: &TrackerConfig) {
    println!("\n## Processor cost\n");
    println!(
        "60 s of the generated melody, fed in device-sized blocks; the best of 5 runs and the"
    );
    println!(
        "median, on one thread of this PC (Ryzen 7 9700X; other work may share the machine, so"
    );
    println!("the best run is the cost and the median shows the noise). Block times: p50 / p99 /");
    println!("max of single process() calls over all runs (a 10 ms block holds exactly one");
    println!("analysis; the max includes the OS pre-empting the thread).\n");
    println!(
        "| rate | block | window / FFT | best ms per s of audio | median | share of one core (best) | block p50 / p99 / max (us) |"
    );
    println!("|---|---|---|---|---|---|---|");
    for rate in RATES {
        let x = melody(rate, 60.0);
        for block in [rate as usize / 100, 512] {
            let mut runs = Vec::new();
            let mut blocks = Vec::with_capacity(5 * x.len() / block + 5);
            for _ in 0..5 {
                let mut t = Tracker::with_config(rate, cfg.clone());
                let mut voiced = 0usize;
                let start = Instant::now();
                for b in x.chunks(block) {
                    let s = Instant::now();
                    t.process(b, |e| voiced += usize::from(e.hz.is_some()));
                    blocks.push(s.elapsed().as_secs_f64());
                }
                runs.push(start.elapsed().as_secs_f64() / 60.0);
                std::hint::black_box(voiced);
            }
            let best = percentile(&mut runs.clone(), 0.0);
            let med = percentile(&mut runs, 0.5);
            let t = Tracker::with_config(rate, cfg.clone());
            println!(
                "| {rate} | {block} | {} / {} | {:.3} | {:.3} | {:.3} % | {:.1} / {:.1} / {:.0} |",
                t.window_len(),
                (2 * t.window_len()).next_power_of_two(),
                1e3 * best,
                1e3 * med,
                100.0 * best,
                1e6 * percentile(&mut blocks, 0.5),
                1e6 * percentile(&mut blocks, 0.99),
                1e6 * percentile(&mut blocks, 1.0)
            );
        }
    }
}

fn clarity(cfg: &TrackerConfig) {
    // Distribution of raw clarity on held voices (for choosing clarity_on / clarity_off).
    println!("\n## Clarity of held voices (raw, for the thresholds)\n");
    println!("| rate | case | p1 | p5 | median |");
    println!("|---|---|---|---|---|");
    for rate in [16_000, 48_000] {
        for (name, breath) in [
            ("clean", None),
            ("breathy -12 dB", Some(-12.0)),
            ("breathy -6 dB", Some(-6.0)),
        ] {
            let mut c = Vec::new();
            for note in LOW..=HIGH {
                let hz = tv::midi_hz(f64::from(note));
                for vowel in Vowel::ALL {
                    let voice = Voice {
                        vowel,
                        breath_db: breath,
                        seed: note as u64,
                        ..Voice::default()
                    };
                    let f = |t: f64| tv::vibrato_hz(hz, 50.0, 5.5, t, 0.2);
                    let x = tv::sing(rate, 1.2, voice, |t| (0.2..1.0).contains(&t).then(|| f(t)));
                    let ev = track(rate, cfg, &x);
                    c.extend(
                        ev.iter()
                            .filter(|e| (0.3..0.95).contains(&secs(e, rate)))
                            .map(|e| f64::from(e.clarity)),
                    );
                }
            }
            println!(
                "| {rate} | {name}, vibrato | {:.3} | {:.3} | {:.3} |",
                percentile(&mut c, 0.01),
                percentile(&mut c, 0.05),
                percentile(&mut c, 0.5)
            );
        }
    }
}

fn bursts(cfg: &TrackerConfig) {
    println!(
        "
## Minimum duration: the shortest periodic burst that shows
"
    );
    println!(
        "A loud (-6 dBFS) held tone of the given length (plus 10 ms ramps each side) alone in"
    );
    println!(
        "silence; the shortest length, in 5 ms steps, that gives any pitched event, over 4 onset"
    );
    println!(
        "positions within a hop (the worst case is shown).
"
    );
    println!("| rate | pitch | shortest burst that shows (ms) |");
    println!("|---|---|---|");
    for rate in [16_000, 48_000] {
        for hz in [65.4, 82.4, 110.0, 150.0, 220.0, 440.0, 880.0] {
            let mut shortest = f64::INFINITY;
            for p in 0..4 {
                let lead = 0.2 + (p * rate as usize / 400) as f64 / f64::from(rate);
                for ms in (0..=200).step_by(5) {
                    let hold = f64::from(ms) / 1000.0;
                    let voice = Voice {
                        level_dbfs: -6.0,
                        ..Voice::default()
                    };
                    let x = tv::note(rate, voice, hz, lead, hold, 0.3);
                    if track(rate, cfg, &x).iter().any(|e| e.hz.is_some()) {
                        shortest = shortest.min(hold);
                        break;
                    }
                }
            }
            println!("| {rate} | {hz} Hz | {:.0} |", 1e3 * shortest);
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let want = |s: &str| args.is_empty() || args.iter().any(|a| a == s);
    let mut cfg = TrackerConfig::default();
    // Overrides for exploring the settings (not used for the recorded numbers).
    let env = |k: &str| std::env::var(k).ok().and_then(|v| v.parse::<f32>().ok());
    if let Some(v) = env("JK_CLARITY_ON") {
        cfg.clarity_on = v;
    }
    if let Some(v) = env("JK_CLARITY_OFF") {
        cfg.clarity_off = v;
    }
    if let Some(v) = env("JK_PEAK_K") {
        cfg.peak_threshold = v;
    }
    if let Some(v) = env("JK_MIN_RUN") {
        cfg.min_voiced_frames = v as u32;
    }
    if let Some(v) = env("JK_MAX_STEP") {
        cfg.max_step_cents = v;
    }
    // Any other setting: JK_SET="field=value,field=value".
    if let Ok(list) = std::env::var("JK_SET") {
        for kv in list.split(',').filter(|s| !s.is_empty()) {
            tracker_set::set_field(&mut cfg, kv);
        }
    }
    println!("Tracker settings: {cfg:?}");
    if want("steady") {
        steady(&cfg);
    }
    if want("vibrato") {
        vibrato(&cfg);
    }
    if want("slides") {
        slides(&cfg);
    }
    if want("breathy") {
        breathy(&cfg);
    }
    if want("noise") {
        noise(&cfg);
    }
    if want("latency") {
        latency(&cfg);
    }
    if want("rates") {
        rates(&cfg);
    }
    if want("bursts") {
        bursts(&cfg);
    }
    if want("cpu") {
        cpu(&cfg);
    }
    if want("clarity") {
        clarity(&cfg);
    }
}

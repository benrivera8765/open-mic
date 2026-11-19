// SPDX-License-Identifier: GPL-3.0-or-later
//! Whole-note checks of the live pitch tracker on generated voices (WORK_ORDER-2 4.4 "octave
//! jumps: none on a steady note"; docs/benchmarks/practice-tracker.md section 13). These are
//! the Stage 0 independent verifier's probes, which found what `tracker_bench` (held part
//! only) does not: octave errors at onsets of high notes, in mildly breathy and jittered
//! voices, and around legato leaps.
//!
//!     cargo run --release -p jk-pitch --example tracker_synth [-- --set field=value ...]
//!
//! Sections (all at 16, 44.1, 48 and 96 kHz unless noted):
//! - `whole`: every semitone C2..C6, vowels ah / ee / oo, 0.6 s notes, clean and with breath
//!   noise at -20 and -12 dB: every pitched event of the whole file more than 50 cents off
//!   the note (onsets and releases included), and how many are octave errors;
//! - `rough`: cycle-to-cycle jitter and shimmer, slow drift, optional breath (16 / 48 kHz):
//!   voiced share of the held part, error, events more than 600 cents off;
//! - `legato`: note A then note B with no gap (intervals 1, 2, 5, 7, 12, -2, -5, -7, -12,
//!   19 semitones from five base notes; targets below C2 skipped), 16 / 48 kHz: events off
//!   both notes, time from the change to the first event on B, and the octave leaps
//!   specifically.

#[path = "tracker_set/mod.rs"]
mod tracker_set;

use jk_pitch::tracker::testvoice::{self as tv, Rng, Voice, Vowel};
use jk_pitch::tracker::{PitchEvent, Tracker, TrackerConfig};

const RATES: [u32; 4] = [16_000, 44_100, 48_000, 96_000];

fn track(rate: u32, cfg: &TrackerConfig, x: &[f32]) -> Vec<PitchEvent> {
    let mut t = Tracker::with_config(rate, cfg.clone());
    let mut out = Vec::with_capacity(x.len() / t.hop_len() + 1);
    for b in x.chunks(rate as usize / 100 + 7) {
        t.process(b, |e| out.push(e));
    }
    out
}

fn secs(e: &PitchEvent, rate: u32) -> f64 {
    e.time as f64 / f64::from(rate)
}

fn quantile(v: &[f64], q: f64) -> f64 {
    if v.is_empty() {
        return f64::NAN;
    }
    let mut s = v.to_vec();
    s.sort_by(|a, b| a.partial_cmp(b).unwrap());
    s[((s.len() - 1) as f64 * q).round() as usize]
}

fn is_octave(c: f64) -> bool {
    let k = (c / 1200.0).round();
    k != 0.0 && (c - 1200.0 * k).abs() < 100.0
}

/// Every pitched event of each whole note more than 50 cents off.
fn whole(cfg: &TrackerConfig) {
    println!("\n## whole notes: events > 50 cents off, onsets and releases included\n");
    println!(
        "| breath | rate | pitched events | > 50 c | of which octave | at onset (< 0.3 s) | held | release (> 0.75 s) | notes with any |"
    );
    println!("|---|---|---|---|---|---|---|---|---|");
    for breath in [None, Some(-20.0), Some(-12.0)] {
        for rate in RATES {
            let (mut n_ev, mut n_bad, mut n_oct, mut notes) = (0usize, 0usize, 0usize, 0usize);
            let mut region = [0usize; 3];
            for vowel in Vowel::ALL {
                for note in 36..=84 {
                    let hz = tv::midi_hz(f64::from(note));
                    let v = Voice {
                        vowel,
                        breath_db: breath,
                        seed: note as u64,
                        ..Voice::default()
                    };
                    let x = tv::note(rate, v, hz, 0.2, 0.6, 0.2);
                    let mut any = false;
                    for e in track(rate, cfg, &x) {
                        let Some(h) = e.hz else { continue };
                        n_ev += 1;
                        let c = tv::cents(f64::from(h), hz);
                        if c.abs() > 50.0 {
                            n_bad += 1;
                            any = true;
                            n_oct += usize::from(is_octave(c));
                            let t = secs(&e, rate);
                            region[if t < 0.3 {
                                0
                            } else if t <= 0.75 {
                                1
                            } else {
                                2
                            }] += 1;
                        }
                    }
                    notes += usize::from(any);
                }
            }
            println!(
                "| {} | {rate} | {n_ev} | {n_bad} | {n_oct} | {} | {} | {} | {notes} / 147 |",
                breath.map_or("none".into(), |b| format!("{b} dB")),
                region[0],
                region[1],
                region[2]
            );
        }
    }
}

/// A rougher, more human voice: jitter, shimmer, drift, optional breath.
fn rough(cfg: &TrackerConfig) {
    println!("\n## rough voices: held part (0.1 s after onset .. 0.05 s before release)\n");
    println!(
        "| rate | jitter | shimmer | breath | voiced | worst note voiced | median / p95 / max (cents) | > 600 c events |"
    );
    println!("|---|---|---|---|---|---|---|---|");
    for rate in [16_000u32, 48_000] {
        for (jit, shim, breath) in [
            (0.005, 0.03, None),
            (0.01, 0.06, None),
            (0.01, 0.06, Some(-20.0)),
            (0.02, 0.10, None),
            (0.02, 0.10, Some(-15.0)),
        ] {
            let (mut ev_n, mut voiced, mut oct) = (0usize, 0usize, 0usize);
            let mut errs = Vec::new();
            let mut worst_cover = 1.0f64;
            for vowel in Vowel::ALL {
                for note in (36..=84).step_by(2) {
                    let hz = tv::midi_hz(f64::from(note));
                    let mut rng = Rng::new(note as u64 * 7 + 1);
                    let ncyc = (2.0 * hz) as usize + 4;
                    let jf: Vec<f64> = (0..ncyc)
                        .map(|_| 1.0 + jit * 1.732 * rng.bipolar())
                        .collect();
                    let sf: Vec<f64> = (0..ncyc)
                        .map(|_| 1.0 + shim * 1.732 * rng.bipolar())
                        .collect();
                    let drift = |t: f64| {
                        10.0 * (2.0 * std::f64::consts::PI * 0.7 * t).sin()
                            + 5.0 * (2.0 * std::f64::consts::PI * 1.9 * t + 1.0).sin()
                    };
                    let truth = move |t: f64| hz * 2f64.powf(drift(t) / 1200.0);
                    let (lead, hold) = (0.2, 1.2);
                    let v = Voice {
                        vowel,
                        breath_db: breath,
                        seed: note as u64,
                        ..Voice::default()
                    };
                    let mut x = tv::sing(rate, lead + hold + 0.2, v, |t| {
                        (t >= lead && t < lead + hold)
                            .then(|| truth(t) * jf[((t - lead) * hz) as usize])
                    });
                    for (i, s) in x.iter_mut().enumerate() {
                        let t = i as f64 / f64::from(rate);
                        if t >= lead {
                            let k = (((t - lead) * hz) as usize).min(ncyc - 1);
                            *s = (f64::from(*s) * sf[k]) as f32;
                        }
                    }
                    let ev = track(rate, cfg, &x);
                    let (mut n, mut vc) = (0usize, 0usize);
                    for e in ev
                        .iter()
                        .filter(|e| (lead + 0.1..=lead + hold - 0.05).contains(&secs(e, rate)))
                    {
                        n += 1;
                        if let Some(h) = e.hz {
                            vc += 1;
                            let c = tv::cents(f64::from(h), truth(secs(e, rate)));
                            oct += usize::from(c.abs() > 600.0);
                            if c.abs() > 600.0 && std::env::var("JK_SHOW").is_ok() {
                                eprintln!(
                                    "rough {rate} {} {:?} t={:.2} hz={h:.0} truth={:.0} clarity={:.2}",
                                    tv::note_name(note),
                                    vowel,
                                    secs(e, rate),
                                    truth(secs(e, rate)),
                                    e.clarity
                                );
                            }
                            errs.push(c.abs());
                        }
                    }
                    ev_n += n;
                    voiced += vc;
                    worst_cover = worst_cover.min(vc as f64 / n as f64);
                }
            }
            println!(
                "| {rate} | {:.1} % | {:.0} % | {} | {:.1} % | {:.1} % | {:.2} / {:.2} / {:.2} | {oct} |",
                jit * 100.0,
                shim * 100.0,
                breath.map_or("none".into(), |b| format!("{b} dB")),
                100.0 * voiced as f64 / ev_n as f64,
                100.0 * worst_cover,
                quantile(&errs, 0.5),
                quantile(&errs, 0.95),
                quantile(&errs, 1.0)
            );
        }
    }
}

/// Legato: A for 0.5 s then B for 0.5 s, no gap.
fn legato(cfg: &TrackerConfig) {
    println!("\n## legato: A for 0.5 s then B for 0.5 s, no gap (5 base notes x 10 intervals)\n");
    println!(
        "| rate | vowel | events off both notes | unvoiced ms in the middle, median / max | change -> first B event ms, median / max | octave leaps up / down: change -> B ms, max |"
    );
    println!("|---|---|---|---|---|---|");
    for rate in [16_000u32, 48_000] {
        for vowel in Vowel::ALL {
            let mut wrong = 0usize;
            let (mut gaps, mut to_b) = (Vec::new(), Vec::new());
            let (mut up, mut down) = (Vec::new(), Vec::new());
            for base in [40, 48, 55, 62, 69] {
                for iv in [1i32, 2, 5, 7, 12, -2, -5, -7, -12, 19] {
                    let (a, b) = (
                        tv::midi_hz(f64::from(base)),
                        tv::midi_hz(f64::from(base + iv)),
                    );
                    // Only targets inside the required range (C2 and up).
                    if b < 65.0 {
                        continue;
                    }
                    let v = Voice {
                        vowel,
                        ..Voice::default()
                    };
                    let x = tv::sing(rate, 1.4, v, |t| {
                        if (0.2..0.7).contains(&t) {
                            Some(a)
                        } else if (0.7..1.2).contains(&t) {
                            Some(b)
                        } else {
                            None
                        }
                    });
                    let ev = track(rate, cfg, &x);
                    let change = (0.7 * f64::from(rate)) as u64;
                    let mut gap = 0usize;
                    let mut first_b = None;
                    for e in &ev {
                        if !(0.35..=1.15).contains(&secs(e, rate)) {
                            continue;
                        }
                        match e.hz {
                            Some(h) => {
                                let (ca, cb) =
                                    (tv::cents(f64::from(h), a), tv::cents(f64::from(h), b));
                                if ca.abs() > 50.0 && cb.abs() > 50.0 {
                                    wrong += 1;
                                }
                                if cb.abs() <= 50.0 && first_b.is_none() && e.ready_at > change {
                                    first_b =
                                        Some((e.ready_at - change) as f64 / f64::from(rate) * 1e3);
                                }
                            }
                            None => gap += 1,
                        }
                    }
                    let tb = first_b.unwrap_or(f64::INFINITY);
                    gaps.push(gap as f64 * 10.0);
                    to_b.push(tb);
                    match iv {
                        12 => up.push(tb),
                        -12 => down.push(tb),
                        _ => {}
                    }
                }
            }
            println!(
                "| {rate} | {} | {wrong} | {:.0} / {:.0} | {:.0} / {:.0} | {:.0} / {:.0} |",
                vowel.name(),
                quantile(&gaps, 0.5),
                quantile(&gaps, 1.0),
                quantile(&to_b, 0.5),
                quantile(&to_b, 1.0),
                quantile(&up, 1.0),
                quantile(&down, 1.0)
            );
        }
    }
}

fn main() {
    let mut cfg = TrackerConfig::default();
    let mut sections: Vec<String> = Vec::new();
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--set" => tracker_set::set_field(&mut cfg, &args.next().unwrap()),
            s => sections.push(s.to_string()),
        }
    }
    let want = |s: &str| sections.is_empty() || sections.iter().any(|a| a == s);
    if want("whole") {
        whole(&cfg);
    }
    if want("rough") {
        rough(&cfg);
    }
    if want("legato") {
        legato(&cfg);
    }
    println!("\nsettings: {cfg:?}");
}

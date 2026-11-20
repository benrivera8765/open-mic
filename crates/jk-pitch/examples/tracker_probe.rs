// SPDX-License-Identifier: GPL-3.0-or-later
//! Probes of the live pitch tracker on generated signals with FRESH SEEDS (WO2 quality round,
//! docs/benchmarks/practice-tracker.md section 22). The robustness verifier found that the
//! whisper fix held only for the seeds it had been checked on, and that the reverb-tail rule
//! cut dry fast fades; these probes take the seed range as an argument, so one range can be
//! used while choosing settings (development) and another, never used before, for the final
//! report (held out).
//!
//!     cargo run --release -p jk-pitch --example tracker_probe -- SECTION [--seeds BASE]
//!         [--set field=value ...]
//!
//! Sections:
//! - `whisper`: per rate (16 / 44.1 / 48 / 96 kHz) 120 x 15 s of whispered speech
//!   (`speech_noise`) and 120 x 15 s of breath, levels -36..-3 dBFS, seeds BASE..BASE+119:
//!   pitched events (must be 0), and each run of them.
//! - `fades`: a dry held note fading at 15..100 dB/s, alone and after three dry phrases
//!   that end abruptly; the same phrases and note through generated rooms (RT60 0.3 / 0.8 /
//!   1.5 s, wet -6 dB): share of the fade shown on the note until it is 24 dB down (dry), and
//!   the time the marker stays on after the dry note has ended (reverberant: the tail).
//!   Also the verifier's other level probes (subito piano, legato to a quieter note, a
//!   tremolo), which must stay at 100 %.
//! - `handling`: handling thumps (40..200 Hz) and popped plosives at many levels and seeds,
//!   alone and between two sung low notes: pitched events outside the notes (must be 0).
//! - `low`: low sung notes C2..C3 (every semitone, three vowels, plain / breathy / rough),
//!   share of the held part shown on the note and the onset delay.

#[path = "tracker_set/mod.rs"]
mod tracker_set;

use jk_pitch::tracker::rig;
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

/// Runs of consecutive pitched events: (start s, length in events, median Hz).
fn runs(ev: &[PitchEvent], rate: u32) -> Vec<(f64, usize, f32)> {
    let mut out = Vec::new();
    let mut cur: Vec<(f64, f32)> = Vec::new();
    for e in ev.iter().chain(std::iter::once(&PitchEvent {
        time: 0,
        ready_at: 0,
        hz: None,
        clarity: 0.0,
        level_dbfs: -200.0,
    })) {
        match e.hz {
            Some(h) => cur.push((secs(e, rate), h)),
            None if !cur.is_empty() => {
                let mut hz: Vec<f32> = cur.iter().map(|c| c.1).collect();
                hz.sort_by(|a, b| a.total_cmp(b));
                out.push((cur[0].0, cur.len(), hz[hz.len() / 2]));
                cur.clear();
            }
            None => {}
        }
    }
    out
}

fn whisper(cfg: &TrackerConfig, base: u64) {
    println!("\n## whisper and breath, seeds {base}..{}\n", base + 119);
    println!("| rate | whispered speech: pitched events (runs) | breath: pitched events (runs) |");
    println!("|---|---|---|");
    for rate in RATES {
        let res: Vec<(usize, usize, usize, usize, Vec<String>)> = std::thread::scope(|sc| {
            let hs: Vec<_> = (0..8u64)
                .map(|w| {
                    let cfg = cfg.clone();
                    sc.spawn(move || {
                        let (mut sp, mut spr, mut br, mut brr) = (0, 0, 0, 0);
                        let mut notes = Vec::new();
                        for i in 0..15u64 {
                            let seed = base + w * 15 + i;
                            let lvl = -36.0 + 33.0 * ((seed * 7 % 11) as f64) / 10.0;
                            let x = tv::speech_noise(rate, 15.0, lvl, seed * 29 + 11);
                            let ev = track(rate, &cfg, &x);
                            for (t, n, hz) in runs(&ev, rate) {
                                sp += n;
                                spr += 1;
                                notes.push(format!(
                                    "whisper seed {seed} {lvl:.1} dBFS at {t:.2} s: {n} events at {hz:.0} Hz"
                                ));
                            }
                            let y = tv::breath(rate, 15.0, lvl - 3.0, seed * 41 + 17);
                            let ev = track(rate, &cfg, &y);
                            for (t, n, hz) in runs(&ev, rate) {
                                br += n;
                                brr += 1;
                                notes.push(format!(
                                    "breath seed {seed} {lvl:.1} dBFS at {t:.2} s: {n} events at {hz:.0} Hz"
                                ));
                            }
                        }
                        (sp, spr, br, brr, notes)
                    })
                })
                .collect();
            hs.into_iter().map(|h| h.join().unwrap()).collect()
        });
        let sum = |k: usize| -> usize {
            res.iter()
                .map(|r| match k {
                    0 => r.0,
                    1 => r.1,
                    2 => r.2,
                    _ => r.3,
                })
                .sum()
        };
        println!(
            "| {rate} | {} ({}) | {} ({}) |",
            sum(0),
            sum(1),
            sum(2),
            sum(3)
        );
        for r in &res {
            for n in &r.4 {
                eprintln!("  {rate} Hz {n}");
            }
        }
    }
}

/// Gain (dB) applied over time.
fn shape(x: &mut [f32], rate: u32, g: impl Fn(f64) -> f64) {
    for (i, v) in x.iter_mut().enumerate() {
        let t = i as f64 / f64::from(rate);
        *v *= 10f64.powf(g(t) / 20.0) as f32;
    }
}

/// Share of events described in [a, b) within 50 cents of `hz`, and the longest gap (ms).
fn shown(ev: &[PitchEvent], rate: u32, a: f64, b: f64, hz: f64) -> (f64, usize) {
    let (mut n, mut ok, mut run, mut worst) = (0, 0, 0usize, 0usize);
    for e in ev {
        let t = secs(e, rate);
        if t < a || t >= b {
            continue;
        }
        n += 1;
        if e.hz.is_some_and(|h| tv::cents(f64::from(h), hz).abs() <= 50.0) {
            ok += 1;
            run = 0;
        } else {
            run += 1;
            worst = worst.max(run);
        }
    }
    (100.0 * ok as f64 / n.max(1) as f64, worst * 10)
}

/// Time (ms) the marker stays on after `end` (the dry note's end): the last pitched event
/// described after `end`, before `limit`.
fn linger(ev: &[PitchEvent], rate: u32, end: f64, limit: f64) -> f64 {
    ev.iter()
        .filter(|e| e.hz.is_some() && (end..limit).contains(&secs(e, rate)))
        .map(|e| secs(e, rate) - end)
        .fold(0.0, f64::max)
        * 1000.0
}

/// Three phrases of four notes each (0.35 s per note, legato), each ending abruptly, with
/// 0.4 s rests, from `t0`; returns the end time.
fn phrases(hz0: f64) -> (impl Fn(f64) -> Option<f64>, f64) {
    let steps = [0i32, 2, 4, 2];
    let f = move |t: f64| -> Option<f64> {
        if !(0.2..4.6).contains(&t) {
            return None;
        }
        let p = t - 0.2;
        let phrase = (p / 1.8).floor();
        let q = p - phrase * 1.8;
        if phrase >= 3.0 || q >= 1.4 {
            return None;
        }
        let k = (q / 0.35).floor() as usize;
        Some(hz0 * 2f64.powf(f64::from(steps[k.min(3)]) / 12.0))
    };
    (f, 0.2 + 3.0 * 1.8)
}

fn fades(cfg: &TrackerConfig) {
    let notes = [45i32, 52, 57, 64, 69, 76]; // A2 E3 A3 E4 A4 E5
    for rate in [16_000u32, 48_000] {
        let mut rows: Vec<(String, f64, usize, usize)> = Vec::new();
        let mut add = |name: String, s: f64, g: usize| {
            if let Some(r) = rows.iter_mut().find(|r| r.0 == name) {
                r.1 += s;
                r.2 = r.2.max(g);
                r.3 += 1;
            } else {
                rows.push((name, s, g, 1));
            }
        };
        for vowel in Vowel::ALL {
            for &m in &notes {
                let hz = tv::midi_hz(f64::from(m));
                let v = Voice {
                    vowel,
                    level_dbfs: -12.0,
                    ..Voice::default()
                };
                // Diminuendo, alone (as the verifier's probe) and after three dry phrases:
                // the held note starts at T, fades from T + 0.8 s at R dB/s to -30 dB.
                for context in [false, true] {
                    let (ph, ph_end) = phrases(hz * 0.9);
                    let t0 = if context { ph_end + 0.3 } else { 0.2 };
                    for rate_db in [15.0, 30.0, 45.0, 60.0, 100.0] {
                        let len = t0 + 3.6;
                        let fade_at = t0 + 0.8;
                        let mut x = tv::sing(rate, len, v, |t| {
                            if (t0..t0 + 3.2).contains(&t) {
                                Some(hz)
                            } else if context {
                                ph(t)
                            } else {
                                None
                            }
                        });
                        shape(&mut x, rate, |t| -(rate_db * (t - fade_at)).clamp(0.0, 30.0));
                        let ev = track(rate, cfg, &x);
                        let end = fade_at + 24.0 / rate_db;
                        let (s, g) = shown(&ev, rate, fade_at, end, hz);
                        let ctx = if context { "after dry phrases" } else { "alone" };
                        add(format!("dry diminuendo {rate_db} dB/s, {ctx}"), s, g);
                    }
                }
                // Subito piano, legato to a quieter note, tremolo (the verifier's probes).
                for drop in [12.0, 20.0, 26.0] {
                    let mut x = tv::sing(rate, 1.8, v, |t| (0.2..1.6).contains(&t).then_some(hz));
                    shape(&mut x, rate, |t| -drop * ((t - 0.8) / 0.02).clamp(0.0, 1.0));
                    let ev = track(rate, cfg, &x);
                    let (s, g) = shown(&ev, rate, 0.85, 1.5, hz);
                    add(format!("subito -{drop} dB, same pitch"), s, g);
                }
                for iv in [1i32, 2, -2, 5, -5] {
                    let hb = tv::midi_hz(f64::from(m + iv));
                    let mut x = tv::sing(rate, 1.8, v, |t| {
                        if (0.2..0.8).contains(&t) {
                            Some(hz)
                        } else if (0.8..1.6).contains(&t) {
                            Some(hb)
                        } else {
                            None
                        }
                    });
                    shape(&mut x, rate, |t| -20.0 * ((t - 0.8) / 0.03).clamp(0.0, 1.0));
                    let ev = track(rate, cfg, &x);
                    let (s, g) = shown(&ev, rate, 0.85, 1.5, hb);
                    add(format!("legato {iv:+} st to -20 dB"), s, g);
                }
                {
                    let mut x = tv::sing(rate, 2.4, v, |t| (0.2..2.2).contains(&t).then_some(hz));
                    shape(&mut x, rate, |t| {
                        3.0 * (2.0 * std::f64::consts::PI * 5.5 * t).sin()
                            - 3.0
                            - (20.0 * (t - 1.0)).clamp(0.0, 30.0)
                    });
                    let ev = track(rate, cfg, &x);
                    let (s, g) = shown(&ev, rate, 1.0, 2.0, hz);
                    add("tremolo 6 dB + fade 20 dB/s".into(), s, g);
                }
            }
        }
        println!("\n## level probes, dry, {rate} Hz\n");
        println!("| probe | mean share shown on the note % | worst gap ms |");
        println!("|---|---|---|");
        for (name, s, g, n) in &rows {
            println!("| {name} | {:.1} | {g} |", s / *n as f64);
        }
        // Reverberant: the same phrases and a held note that stops abruptly (no fade),
        // through rooms; the marker after the dry note's end is the room's tail.
        println!("\n## reverberant phrases, {rate} Hz: marker after the dry note ended\n");
        println!(
            "| room | tail shown after the last note, median / max ms | held note shown % (its last 0.3 s) |"
        );
        println!("|---|---|---|");
        for (rt60, name) in [(0.0, "dry"), (0.3, "RT60 0.3 s"), (0.8, "RT60 0.8 s"), (1.5, "RT60 1.5 s")] {
            let mut tails = Vec::new();
            let mut held = 0.0;
            let mut n = 0;
            for vowel in Vowel::ALL {
                for &m in &notes {
                    let hz = tv::midi_hz(f64::from(m));
                    let v = Voice {
                        vowel,
                        level_dbfs: -12.0,
                        ..Voice::default()
                    };
                    let (ph, ph_end) = phrases(hz * 0.9);
                    let t0 = ph_end + 0.3;
                    let x = tv::sing(rate, t0 + 2.6, v, |t| {
                        if (t0..t0 + 1.0).contains(&t) {
                            Some(hz)
                        } else {
                            ph(t)
                        }
                    });
                    let x = if rt60 > 0.0 {
                        rig::reverb(&x, rate, rt60, -6.0, (m as u64) * 7 + 3)
                    } else {
                        x
                    };
                    let ev = track(rate, cfg, &x);
                    tails.push(linger(&ev, rate, t0 + 1.0 + tv::RAMP_S, t0 + 2.5));
                    held += shown(&ev, rate, t0 + 0.7, t0 + 1.0, hz).0;
                    n += 1;
                }
            }
            tails.sort_by(|a, b| a.partial_cmp(b).unwrap());
            println!(
                "| {name} | {:.0} / {:.0} | {:.1} |",
                tails[tails.len() / 2],
                tails[tails.len() - 1],
                held / f64::from(n)
            );
        }
    }
}

fn handling(cfg: &TrackerConfig, base: u64) {
    println!("\n## handling noise, seeds {base}..\n");
    println!("| rate | thumps alone: events | pops alone: events | between two sung notes: events outside the notes |");
    println!("|---|---|---|---|");
    for rate in RATES {
        let mut rng = Rng::new(base);
        let (mut th, mut po, mut mixed) = (0, 0, 0);
        for k in 0..60u64 {
            let hz = 40.0 + 160.0 * rng.unit();
            let peak = -1.0 - 17.0 * rng.unit();
            let x = tv::thump(rate, 0.5, 0.1, hz, peak);
            th += track(rate, cfg, &x).iter().filter(|e| e.hz.is_some()).count();
            let y = tv::plosive(rate, 0.5, 0.1, peak, base + k);
            po += track(rate, cfg, &y).iter().filter(|e| e.hz.is_some()).count();
            // A low sung note, a thump or a pop 0.15 s after it ends, another note.
            let note = 36.0 + 12.0 * rng.unit();
            let nh = tv::midi_hz(note.round());
            let v = Voice {
                level_dbfs: -18.0,
                vowel: Vowel::ALL[(k % 3) as usize],
                seed: base + k,
                ..Voice::default()
            };
            let mut z = tv::sing(rate, 1.6, v, |t| {
                ((0.1..0.5).contains(&t) || (1.0..1.4).contains(&t)).then_some(nh)
            });
            let burst = if k % 2 == 0 {
                tv::thump(rate, 0.3, 0.0, hz, peak)
            } else {
                tv::plosive(rate, 0.3, 0.0, peak, base + k)
            };
            let at = (0.65 * f64::from(rate)) as usize;
            for (d, s) in z[at..].iter_mut().zip(&burst) {
                *d += *s;
            }
            let ev = track(rate, cfg, &z);
            mixed += ev
                .iter()
                .filter(|e| e.hz.is_some() && (0.6..0.95).contains(&secs(e, rate)))
                .count();
        }
        println!("| {rate} | {th} | {po} | {mixed} |");
    }
}

fn low(cfg: &TrackerConfig) {
    println!("\n## low sung notes C2..C3 (0.6 s notes, -20 dBFS)\n");
    println!("| rate | voice | held part shown on the note % | onset to first event on the note, median / max ms |");
    println!("|---|---|---|---|");
    for rate in [16_000u32, 48_000] {
        for (name, breath, jitter) in [("plain", None, false), ("breathy -15 dB", Some(-15.0), false), ("rough", None, true)] {
            let (mut ok, mut n) = (0usize, 0usize);
            let mut delays = Vec::new();
            for note in 36..=48 {
                for vowel in Vowel::ALL {
                    let hz = tv::midi_hz(f64::from(note));
                    let v = Voice {
                        vowel,
                        breath_db: breath,
                        seed: note as u64,
                        ..Voice::default()
                    };
                    // Rough: a random pitch drift (cents) sampled every millisecond.
                    let mut rng = Rng::new(note as u64 * 7 + vowel as u64);
                    let mut wob = 0.0f64;
                    let drift: Vec<f64> = (0..1001)
                        .map(|_| {
                            wob = 0.995 * wob + 3.0 * rng.bipolar();
                            wob
                        })
                        .collect();
                    let x = tv::sing(rate, 1.0, v, |t| {
                        if !(0.2..0.8).contains(&t) {
                            return None;
                        }
                        if jitter {
                            let k = ((t * 1000.0) as usize).min(1000);
                            Some(hz * 2f64.powf(drift[k] / 1200.0))
                        } else {
                            Some(hz)
                        }
                    });
                    let ev = track(rate, cfg, &x);
                    for e in ev.iter().filter(|e| (0.3..0.75).contains(&secs(e, rate))) {
                        n += 1;
                        ok += usize::from(e.hz.is_some_and(|h| tv::cents(f64::from(h), hz).abs() <= 50.0));
                    }
                    let onset = (0.2 * f64::from(rate)) as u64;
                    let d = ev
                        .iter()
                        .find(|e| e.ready_at > onset && e.hz.is_some_and(|h| tv::cents(f64::from(h), hz).abs() <= 50.0))
                        .map_or(500.0, |e| (e.ready_at - onset) as f64 / f64::from(rate) * 1000.0);
                    delays.push(d);
                }
            }
            delays.sort_by(|a, b| a.partial_cmp(b).unwrap());
            println!(
                "| {rate} | {name} | {:.1} | {:.0} / {:.0} |",
                100.0 * ok as f64 / n as f64,
                delays[delays.len() / 2],
                delays[delays.len() - 1]
            );
        }
    }
}

fn main() {
    let mut cfg = TrackerConfig::default();
    let mut sections = Vec::new();
    let mut base = 7000u64;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--set" => tracker_set::set_field(&mut cfg, &args.next().unwrap()),
            "--seeds" => base = args.next().unwrap().parse().unwrap(),
            s => sections.push(s.to_string()),
        }
    }
    if sections.is_empty() {
        sections = vec!["whisper".into(), "fades".into(), "handling".into(), "low".into()];
    }
    for s in &sections {
        match s.as_str() {
            "whisper" => whisper(&cfg, base),
            "fades" => fades(&cfg),
            "handling" => handling(&cfg, base),
            "low" => low(&cfg),
            o => panic!("unknown section {o}"),
        }
    }
    println!("\nsettings: {cfg:?}");
}

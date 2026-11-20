// SPDX-License-Identifier: GPL-3.0-or-later
//! Real-voice evaluation of the live pitch tracker (WORK_ORDER-2 4.4 tracker table, traps
//! 5 / 7 / 14; docs/benchmarks/practice-tracker.md section 13).
//!
//!     cargo run --release -p jk-pitch --example tracker_eval -- [options]
//!
//! Reads the clips and references made by `examples/tracker_eval_prep.py` (outside the
//! repository, `$JK_BUILD/practice-tracker/eval/manifest.tsv`): one channel of real sung
//! voice per clip at its own sample rate, and a 10 ms reference (1 voiced with f0, 0 not
//! voiced, 2 undecided). Prints Markdown tables per dataset.
//!
//! Options:
//! - `--split dev|heldout|all` (default dev): tune on dev only, report held-out.
//! - `--cond NAME[,NAME...]` (default clean): the owner's-rig conditions applied to every
//!   clip before tracking: clean, rev03, rev08, rev15, rev15wet0, comp, comp_heavy,
//!   eq_bright, eq_warm, prox, pops, rig (all of it at once), r16k, r48k, r96k (the clip
//!   resampled by an integer-ratio-free linear-phase sinc; the reference is unchanged).
//! - `--set field=value` (repeatable): a `TrackerConfig` override, for exploration.
//! - `--events DIR`: write each clip's events as CSV.
//! - `--clips`: one line per clip as well.
//! - `--delays`: the distribution of the onset delays as well (share of onsets per delay
//!   band, against WO2's 100 ms ceiling; "later" includes onsets never shown on the note).
//! - `--tsv PATH`: the tables as tab-separated values.
//! - `--manifest PATH`; `--only TEXT` keeps the clips whose id contains TEXT.
//!
//! Metrics (frames on the reference's 10 ms grid, each tracker event matched to the nearest
//! reference frame by its described `time`):
//! - VR, voicing recall: reference-voiced frames the tracker voices, over "stable" frames
//!   (the reference has the same flag 20 ms either side, so a 10 ms disagreement about where
//!   a note starts is not counted);
//! - VFA, voicing false alarm: stable reference-unvoiced frames the tracker voices;
//! - RPA, raw pitch accuracy: reference-voiced frames (all of them) the tracker voices
//!   within 50 cents (voicing and pitch together, as the marker shows them);
//! - Acc: frames both voice, within 50 cents;
//! - Oct: frames both voice, off by an octave or more (|error| within 100 cents of a
//!   non-zero multiple of 1200, or more than 1100 cents) - share of both-voiced frames;
//! - Gross: both voiced, more than 100 cents off but not an octave error;
//! - flips: octave jumps of the marker inside a voiced run while the reference holds its
//!   pitch (consecutive voiced events 600 cents or more apart, reference moving < 300 cents);
//! - onsets: a reference onset is a voiced frame after at least 100 ms of reference
//!   "unvoiced" and before at least 150 ms of "voiced"; delay = from the onset to the
//!   `ready_at` of the first event within 50 cents of the note (median of its first 150 ms);
//!   "found" = within 100 ms; "flash" = a pitched event an octave or more off whose described
//!   time lies within the first 150 ms of the note.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use jk_pitch::tracker::rig::{self, Compressor};
use jk_pitch::tracker::{PitchEvent, Tracker, TrackerConfig};

#[path = "tracker_set/mod.rs"]
mod tracker_set;
use tracker_set::set_field;

struct Clip {
    dataset: String,
    split: String,
    id: String,
    rate: u32,
    clip: PathBuf,
    reference: PathBuf,
}

#[derive(Clone, Copy)]
struct RefFrame {
    f0: f64,
    flag: u8,
}

#[derive(Default, Clone, Debug)]
struct Stats {
    clips: usize,
    seconds: f64,
    /// Stable reference-voiced frames and how many the tracker voiced.
    vr_n: usize,
    vr_hit: usize,
    /// Stable reference-unvoiced frames and how many the tracker voiced.
    vfa_n: usize,
    vfa_hit: usize,
    /// All reference-voiced frames, and those voiced within 50 cents.
    rpa_n: usize,
    rpa_hit: usize,
    /// Both voiced: total, within 50 c, octave errors, gross non-octave errors.
    both: usize,
    acc: usize,
    oct: usize,
    /// Octave errors with the tracker ABOVE the reference (octave-high).
    oct_hi: usize,
    gross: usize,
    /// Octave jumps of the marker inside a voiced run where the reference holds its pitch
    /// (consecutive voiced events >= 600 cents apart, reference moving < 300 cents).
    flips: usize,
    onsets: usize,
    found: usize,
    flashes: usize,
    flash_onsets: usize,
    delays_ms: Vec<f64>,
}

impl Stats {
    fn add(&mut self, o: &Stats) {
        self.clips += o.clips;
        self.seconds += o.seconds;
        self.vr_n += o.vr_n;
        self.vr_hit += o.vr_hit;
        self.vfa_n += o.vfa_n;
        self.vfa_hit += o.vfa_hit;
        self.rpa_n += o.rpa_n;
        self.rpa_hit += o.rpa_hit;
        self.both += o.both;
        self.acc += o.acc;
        self.oct += o.oct;
        self.oct_hi += o.oct_hi;
        self.gross += o.gross;
        self.flips += o.flips;
        self.onsets += o.onsets;
        self.found += o.found;
        self.flashes += o.flashes;
        self.flash_onsets += o.flash_onsets;
        self.delays_ms.extend(&o.delays_ms);
    }
}

fn pct(a: usize, b: usize) -> f64 {
    if b == 0 {
        f64::NAN
    } else {
        100.0 * a as f64 / b as f64
    }
}

fn quantile(v: &[f64], q: f64) -> f64 {
    if v.is_empty() {
        return f64::NAN;
    }
    let mut s = v.to_vec();
    s.sort_by(|a, b| a.partial_cmp(b).unwrap());
    s[((s.len() - 1) as f64 * q).round() as usize]
}

fn cents(a: f64, b: f64) -> f64 {
    1200.0 * (a / b).log2()
}

fn is_octave(c: f64) -> bool {
    let k = (c / 1200.0).round();
    c.abs() > 1100.0 || (k != 0.0 && (c - 1200.0 * k).abs() < 100.0)
}

fn read_manifest(path: &Path) -> Vec<Clip> {
    let text = std::fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("{}: {e} (run tracker_eval_prep.py first)", path.display()));
    text.lines()
        .skip(1)
        .filter(|l| !l.trim().is_empty())
        .map(|l| {
            let f: Vec<&str> = l.split('\t').collect();
            Clip {
                dataset: f[0].into(),
                split: f[1].into(),
                id: f[2].into(),
                rate: f[4].parse().unwrap(),
                clip: f[5].into(),
                reference: f[6].trim().into(),
            }
        })
        .collect()
}

fn read_f32(path: &Path) -> Vec<f32> {
    let b = std::fs::read(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    b.as_chunks::<4>()
        .0
        .iter()
        .map(|c| f32::from_le_bytes(*c))
        .collect()
}

fn read_ref(path: &Path) -> Vec<RefFrame> {
    std::fs::read_to_string(path)
        .unwrap()
        .lines()
        .skip(1)
        .map(|l| {
            let f: Vec<&str> = l.split(',').collect();
            RefFrame {
                f0: f[1].parse().unwrap(),
                flag: f[2].trim().parse().unwrap(),
            }
        })
        .collect()
}

/// Band-limited resampling by windowed sinc (Blackman, 32 zero crossings), for the
/// sample-rate conditions. The kernel is tabulated at 1/512 of a tap and interpolated
/// linearly (error far below the 24-bit noise floor), which makes it fast enough for an
/// hour of audio.
fn resample(x: &[f32], from: u32, to: u32) -> Vec<f32> {
    const PHASES: f64 = 512.0;
    let ratio = f64::from(to) / f64::from(from);
    let n = (x.len() as f64 * ratio).floor() as usize;
    let cutoff = ratio.min(1.0) * 0.97;
    let half = 32.0 / cutoff;
    let pi = std::f64::consts::PI;
    let kernel = |d: f64| {
        if d.abs() >= half {
            return 0.0;
        }
        let w = 0.42 + 0.5 * (pi * d / half).cos() + 0.08 * (2.0 * pi * d / half).cos();
        let a = pi * d * cutoff;
        let sinc = if a.abs() < 1e-9 { 1.0 } else { a.sin() / a };
        cutoff * sinc * w
    };
    let size = (2.0 * half * PHASES).ceil() as usize + 2;
    let table: Vec<f64> = (0..size)
        .map(|k| kernel(k as f64 / PHASES - half))
        .collect();
    (0..n)
        .map(|i| {
            let c = i as f64 / ratio;
            let lo = (c - half).ceil().max(0.0) as usize;
            let hi = ((c + half).floor() as usize).min(x.len() - 1);
            let mut acc = 0.0;
            for (j, &s) in x[lo..=hi].iter().enumerate() {
                let pos = ((lo + j) as f64 - c + half) * PHASES;
                let k = pos.floor() as usize;
                let frac = pos - k as f64;
                let w = table[k] + (table[k + 1] - table[k]) * frac;
                acc += f64::from(s) * w;
            }
            acc as f32
        })
        .collect()
}

/// Applies a rig condition. Returns the signal and its rate.
fn condition(name: &str, x: Vec<f32>, rate: u32, seed: u64) -> (Vec<f32>, u32) {
    match name {
        "clean" => (x, rate),
        "rev03" => (rig::reverb(&x, rate, 0.3, -6.0, seed), rate),
        "rev08" => (rig::reverb(&x, rate, 0.8, -6.0, seed), rate),
        "rev15" => (rig::reverb(&x, rate, 1.5, -6.0, seed), rate),
        "rev15wet0" => (rig::reverb(&x, rate, 1.5, 0.0, seed), rate),
        "comp" => {
            let mut y = x;
            rig::add_noise_floor(&mut y, rate, -65.0, seed);
            (rig::compress(&y, rate, Compressor::MODERATE), rate)
        }
        "comp_heavy" => {
            let mut y = x;
            rig::add_noise_floor(&mut y, rate, -65.0, seed);
            (rig::compress(&y, rate, Compressor::HEAVY), rate)
        }
        "eq_bright" => (rig::mixer_eq(&x, rate, true), rate),
        "eq_warm" => (rig::mixer_eq(&x, rate, false), rate),
        "prox" => (rig::proximity(&x, rate, 12.0), rate),
        "pops" => {
            let mut y = rig::proximity(&x, rate, 12.0);
            let minutes = y.len() as f64 / f64::from(rate) / 60.0;
            rig::add_handling(
                &mut y,
                rate,
                (30.0 * minutes) as usize,
                (10.0 * minutes) as usize,
                seed,
            );
            (y, rate)
        }
        "rig" => {
            // The whole chain: close-up dynamic mic (proximity, pops, thumps), the booth's
            // noise floor, a bright channel EQ, heavy compression, the mixer's reverb.
            let mut y = rig::proximity(&x, rate, 12.0);
            let minutes = y.len() as f64 / f64::from(rate) / 60.0;
            rig::add_handling(
                &mut y,
                rate,
                (30.0 * minutes) as usize,
                (10.0 * minutes) as usize,
                seed,
            );
            rig::add_noise_floor(&mut y, rate, -65.0, seed + 1);
            let y = rig::mixer_eq(&y, rate, true);
            let y = rig::compress(&y, rate, Compressor::HEAVY);
            (rig::reverb(&y, rate, 1.0, -8.0, seed + 2), rate)
        }
        "r16k" => (resample(&x, rate, 16_000), 16_000),
        "r48k" => (resample(&x, rate, 48_000), 48_000),
        "r96k" => (resample(&x, rate, 96_000), 96_000),
        other => panic!("unknown condition {other}"),
    }
}

fn track(rate: u32, cfg: &TrackerConfig, x: &[f32]) -> Vec<PitchEvent> {
    let mut t = Tracker::with_config(rate, cfg.clone());
    let mut out = Vec::with_capacity(x.len() / t.hop_len() + 1);
    for block in x.chunks(rate as usize / 100 + 7) {
        t.process(block, |e| out.push(e));
    }
    out
}

/// Scores one clip's events against its reference.
fn score(ev: &[PitchEvent], rate: u32, r: &[RefFrame], seconds: f64) -> Stats {
    let mut s = Stats {
        clips: 1,
        seconds,
        ..Stats::default()
    };
    // Tracker pitch per reference frame (None = no event there or not voiced).
    let mut at: Vec<Option<Option<f64>>> = vec![None; r.len()];
    for e in ev {
        let t = e.time as f64 / f64::from(rate);
        let k = (t / 0.01).round();
        if k < 0.0 || (t - k * 0.01).abs() > 0.0051 {
            continue;
        }
        let k = k as usize;
        if k < r.len() {
            at[k] = Some(e.hz.map(f64::from));
        }
    }
    let stable = |k: usize| {
        let lo = k.saturating_sub(2);
        let hi = (k + 2).min(r.len() - 1);
        (lo..=hi).all(|j| r[j].flag == r[k].flag)
    };
    for k in 0..r.len() {
        let Some(tr) = at[k] else { continue };
        match r[k].flag {
            1 => {
                s.rpa_n += 1;
                if stable(k) {
                    s.vr_n += 1;
                    s.vr_hit += usize::from(tr.is_some());
                }
                if let Some(hz) = tr {
                    let c = cents(hz, r[k].f0);
                    s.both += 1;
                    if c.abs() <= 50.0 {
                        s.acc += 1;
                        s.rpa_hit += 1;
                    } else if is_octave(c) {
                        s.oct += 1;
                        s.oct_hi += usize::from(c > 0.0);
                    } else if c.abs() > 100.0 {
                        s.gross += 1;
                    }
                }
            }
            0 if stable(k) => {
                s.vfa_n += 1;
                s.vfa_hit += usize::from(tr.is_some());
            }
            _ => {}
        }
    }
    // Octave jumps inside voiced runs.
    for k in 1..r.len() {
        if let (Some(Some(a)), Some(Some(b))) = (at[k - 1], at[k]) {
            let ref_holds =
                r[k - 1].flag == 1 && r[k].flag == 1 && cents(r[k].f0, r[k - 1].f0).abs() < 300.0;
            if cents(b, a).abs() >= 600.0 && ref_holds {
                s.flips += 1;
            }
        }
    }
    // Onsets.
    let k_ready: Vec<(f64, f64, Option<f64>)> = ev
        .iter()
        .map(|e| {
            (
                e.time as f64 / f64::from(rate),
                e.ready_at as f64 / f64::from(rate),
                e.hz.map(f64::from),
            )
        })
        .collect();
    for k in 10..r.len().saturating_sub(15) {
        if r[k].flag != 1
            || !(k - 10..k).all(|j| r[j].flag == 0)
            || !(k..k + 15).all(|j| r[j].flag == 1)
        {
            continue;
        }
        let onset = k as f64 * 0.01;
        let mut f: Vec<f64> = (k..k + 15).map(|j| r[j].f0).collect();
        f.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let note = f[7];
        s.onsets += 1;
        let first = k_ready.iter().find(|(_, ready, hz)| {
            *ready > onset - 0.02 && hz.is_some_and(|h| cents(h, note).abs() <= 50.0)
        });
        if let Some((_, ready, _)) = first {
            let d = (ready - onset) * 1000.0;
            if d <= 100.0 {
                s.found += 1;
            }
            s.delays_ms.push(d.min(500.0));
        } else {
            s.delays_ms.push(500.0);
        }
        let flashes = k_ready
            .iter()
            .filter(|(t, _, hz)| {
                (onset..onset + 0.15).contains(t) && hz.is_some_and(|h| is_octave(cents(h, note)))
            })
            .count();
        s.flashes += flashes;
        s.flash_onsets += usize::from(flashes > 0);
    }
    s
}

fn row(name: &str, s: &Stats) -> String {
    format!(
        "| {name} | {} | {:.1} | {:.1} | {:.2} | {:.1} | {:.2} | {:.2} ({:.2} high) | {:.2} | {} | {} | {:.0} / {:.0} | {:.1} | {} ({}) |",
        s.clips,
        s.seconds / 60.0,
        pct(s.vr_hit, s.vr_n),
        pct(s.vfa_hit, s.vfa_n),
        pct(s.rpa_hit, s.rpa_n),
        pct(s.acc, s.both),
        pct(s.oct, s.both),
        pct(s.oct_hi, s.both),
        pct(s.gross, s.both),
        s.flips,
        s.onsets,
        quantile(&s.delays_ms, 0.5),
        quantile(&s.delays_ms, 0.9),
        pct(s.found, s.onsets),
        s.flashes,
        s.flash_onsets,
    )
}

/// One row of the onset-delay distribution: share of onsets per delay band (ms).
fn delay_row(name: &str, s: &Stats) -> String {
    let n = s.delays_ms.len().max(1) as f64;
    let share = |lo: f64, hi: f64| {
        100.0 * s.delays_ms.iter().filter(|&&d| d > lo && d <= hi).count() as f64 / n
    };
    format!(
        "| {name} | {} | {:.1} | {:.1} | {:.1} | {:.1} | {:.1} | {:.1} | {:.0} / {:.0} / {:.0} |",
        s.delays_ms.len(),
        share(f64::NEG_INFINITY, 50.0),
        share(50.0, 75.0),
        share(75.0, 100.0),
        share(100.0, 150.0),
        share(150.0, 250.0),
        share(250.0, f64::INFINITY),
        quantile(&s.delays_ms, 0.5),
        quantile(&s.delays_ms, 0.75),
        quantile(&s.delays_ms, 0.9),
    )
}

fn main() {
    let mut split = "dev".to_string();
    let mut conds = vec!["clean".to_string()];
    let mut cfg = TrackerConfig::default();
    let mut events_dir: Option<PathBuf> = None;
    let mut per_clip = false;
    let mut delays = false;
    let mut tsv_out: Option<PathBuf> = None;
    let mut manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../../jukeoke-build/practice-tracker/eval/manifest.tsv");
    let mut only: Option<String> = None;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--split" => split = args.next().unwrap(),
            "--cond" => conds = args.next().unwrap().split(',').map(String::from).collect(),
            "--set" => set_field(&mut cfg, &args.next().unwrap()),
            "--events" => events_dir = Some(args.next().unwrap().into()),
            "--clips" => per_clip = true,
            "--delays" => delays = true,
            "--tsv" => tsv_out = Some(args.next().unwrap().into()),
            "--manifest" => manifest = args.next().unwrap().into(),
            "--only" => only = Some(args.next().unwrap()),
            other => panic!("unknown argument {other}"),
        }
    }
    let clips: Vec<Clip> = read_manifest(&manifest)
        .into_iter()
        .filter(|c| split == "all" || c.split == split)
        .filter(|c| only.as_ref().is_none_or(|o| c.id.contains(o.as_str())))
        .collect();
    let mut out_tsv = BTreeMap::new();
    for cond in &conds {
        // Clips in parallel (each its own tracker); results gathered in manifest order.
        let next = std::sync::atomic::AtomicUsize::new(0);
        let results: Vec<std::sync::Mutex<Option<Stats>>> =
            clips.iter().map(|_| std::sync::Mutex::new(None)).collect();
        let threads = std::thread::available_parallelism().map_or(4, |n| n.get());
        std::thread::scope(|sc| {
            for _ in 0..threads {
                sc.spawn(|| {
                    loop {
                        let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        if i >= clips.len() {
                            break;
                        }
                        let c = &clips[i];
                        let x = read_f32(&c.clip);
                        let seconds = x.len() as f64 / f64::from(c.rate);
                        let seed =
                            c.id.bytes()
                                .fold(7u64, |h, b| h.wrapping_mul(31).wrapping_add(u64::from(b)));
                        let (y, rate) = condition(cond, x, c.rate, seed);
                        let ev = track(rate, &cfg, &y);
                        if let Some(dir) = &events_dir {
                            std::fs::create_dir_all(dir).unwrap();
                            let mut t = String::from("time_s,ready_s,hz,clarity,level_dbfs\n");
                            for e in &ev {
                                let _ = writeln!(
                                    t,
                                    "{:.4},{:.4},{},{:.4},{:.1}",
                                    e.time as f64 / f64::from(rate),
                                    e.ready_at as f64 / f64::from(rate),
                                    e.hz.map_or("nan".into(), |h| format!("{h:.3}")),
                                    e.clarity,
                                    e.level_dbfs
                                );
                            }
                            std::fs::write(dir.join(format!("{}-{cond}.csv", c.id)), t).unwrap();
                        }
                        let r = read_ref(&c.reference);
                        *results[i].lock().unwrap() = Some(score(&ev, rate, &r, seconds));
                    }
                });
            }
        });
        let mut by_ds: BTreeMap<String, Stats> = BTreeMap::new();
        let mut total = Stats::default();
        println!(
            "\n### split {split}, condition {cond}\n\n| set | clips | min | VR % | VFA % | RPA % | Acc % | Oct % | Gross % | flips | onsets | delay p50 / p90 ms | found <=100 ms % | onset flashes (onsets) |\n|---|---|---|---|---|---|---|---|---|---|---|---|---|"
        );
        for (c, m) in clips.iter().zip(&results) {
            let s = m.lock().unwrap().take().unwrap();
            if per_clip {
                eprintln!("{}", row(&format!("{} {}", c.dataset, c.id), &s));
            }
            by_ds.entry(c.dataset.clone()).or_default().add(&s);
            total.add(&s);
        }
        for (ds, s) in &by_ds {
            println!("{}", row(ds, s));
        }
        println!("{}", row("**all**", &total));
        if delays {
            println!(
                "\nonset delay, split {split}, condition {cond} (share of onsets, %)\n\n\
                 | set | onsets | <= 50 ms | 50-75 | 75-100 | 100-150 | 150-250 | later or never \
                 | p50 / p75 / p90 ms |\n|---|---|---|---|---|---|---|---|---|"
            );
            for (ds, s) in &by_ds {
                println!("{}", delay_row(ds, s));
            }
            println!("{}", delay_row("**all**", &total));
        }
        let mut m = BTreeMap::new();
        for (ds, s) in by_ds {
            m.insert(ds, s);
        }
        m.insert("all".into(), total);
        out_tsv.insert(cond.clone(), m);
    }
    if let Some(p) = tsv_out {
        let mut t = String::from(
            "split\tcondition\tset\tclips\tminutes\tvr\tvfa\trpa\tacc\toct\toct_hi\tgross\tflips\tonsets\tdelay_p50_ms\tdelay_p90_ms\tfound_pct\tflashes\tflash_onsets\n",
        );
        for (c, m) in &out_tsv {
            for (d, s) in m {
                let _ = writeln!(
                    t,
                    "{split}\t{c}\t{d}\t{}\t{:.2}\t{:.3}\t{:.3}\t{:.3}\t{:.3}\t{:.3}\t{:.3}\t{:.3}\t{}\t{}\t{:.1}\t{:.1}\t{:.2}\t{}\t{}",
                    s.clips,
                    s.seconds / 60.0,
                    pct(s.vr_hit, s.vr_n),
                    pct(s.vfa_hit, s.vfa_n),
                    pct(s.rpa_hit, s.rpa_n),
                    pct(s.acc, s.both),
                    pct(s.oct, s.both),
                    pct(s.oct_hi, s.both),
                    pct(s.gross, s.both),
                    s.flips,
                    s.onsets,
                    quantile(&s.delays_ms, 0.5),
                    quantile(&s.delays_ms, 0.9),
                    pct(s.found, s.onsets),
                    s.flashes,
                    s.flash_onsets
                );
            }
        }
        std::fs::write(p, t).unwrap();
    }
    println!("\nsettings: {cfg:?}");
}

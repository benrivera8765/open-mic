// SPDX-License-Identifier: GPL-3.0-or-later
//! The cases of docs/benchmarks/practice-echo.md section 10 that the scenario benchmark
//! (`echo_eval.rs`) does not cover: the mixer's loopback switched off (and on) in the middle
//! of a song, a high voice singing "ee" / "oo" (whose notes vary in level, some under the
//! returned music), and continuous singing from the first second with no instrumental
//! intro, and changes of the echo path (OS volume, the app's master volume, which the
//! engine's output tap includes). Grown from the independent verifier's script of Stage 2
//! (`../jukeoke-build/practice-echo/verify/echo_verify.rs.txt`).
//!
//! ```text
//! cargo run --release -p jk-pitch --example echo_cases -- \
//!     --tracks <dir with ccMixter track folders> --songs ccm-22364,ccm-31416 \
//!     --out <results dir> [--only <substring>]
//! ```
//!
//! Each ccMixter track folder holds instrumental.wav, backing.wav (Creative Commons,
//! evaluation only, never committed). Every voice is generated; nothing touches a
//! microphone and only numbers are written (`cases.jsonl`, one line per case and song).

#![allow(clippy::needless_range_loop)] // index loops over aligned signals read plainly

use std::fmt::Write as _;

use jk_pitch::echo::testpath::{self, ReturnPath};
use jk_pitch::echo::{EchoCanceller, Presence, RemovalQuality, Trust};
use jk_pitch::tracker::Tracker;
use jk_pitch::tracker::testvoice::{self as tv, Rng, Voice, Vowel};

/// Reads a 16/24-bit integer or 32-bit float WAV file: (rate, channels, samples).
#[allow(clippy::chunks_exact_to_as_chunks)] // byte chunks read plainly in a test tool
fn read_wav(path: &str) -> (u32, usize, Vec<f32>) {
    let b = std::fs::read(path).unwrap_or_else(|e| panic!("{path}: {e}"));
    let u16le = |o: usize| u16::from_le_bytes([b[o], b[o + 1]]);
    let u32le = |o: usize| u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]]);
    let mut pos = 12;
    let (mut fmt, mut ch, mut rate, mut bits) = (0u16, 0usize, 0u32, 0u16);
    while pos + 8 <= b.len() {
        let id = &b[pos..pos + 4];
        let len = u32le(pos + 4) as usize;
        let body = pos + 8;
        if id == b"fmt " {
            fmt = u16le(body);
            ch = u16le(body + 2) as usize;
            rate = u32le(body + 4);
            bits = u16le(body + 14);
            if fmt == 0xFFFE {
                fmt = u16le(body + 24);
            }
        } else if id == b"data" {
            let d = &b[body..(body + len).min(b.len())];
            let s: Vec<f32> = match (fmt, bits) {
                (1, 16) => d
                    .chunks_exact(2)
                    .map(|s| i16::from_le_bytes([s[0], s[1]]) as f32 / 32768.0)
                    .collect(),
                (1, 24) => d
                    .chunks_exact(3)
                    .map(|s| (i32::from_le_bytes([0, s[0], s[1], s[2]]) >> 8) as f32 / 8_388_608.0)
                    .collect(),
                (3, 32) => d
                    .chunks_exact(4)
                    .map(|s| f32::from_le_bytes([s[0], s[1], s[2], s[3]]))
                    .collect(),
                _ => panic!("{path}: WAV format {fmt} / {bits} bits"),
            };
            return (rate, ch, s);
        }
        pos = body + len + (len & 1);
    }
    panic!("{path}: no data chunk")
}

/// Stereo excerpt of the backing (instrumental + backing vocals) of a ccMixter track at
/// -14 dBFS mono RMS, as the benchmark plays it.
fn song(dir: &str, start: f64, secs: f64) -> (u32, Vec<f32>) {
    let mut acc: Vec<f32> = Vec::new();
    let mut rate = 0;
    for f in ["instrumental.wav", "backing.wav", "bass.wav", "drums.wav", "other.wav"] {
        // Some ccMixter folders have no instrumental stem: their backing stem is the
        // whole accompaniment.
        if !std::path::Path::new(&format!("{dir}/{f}")).exists() {
            continue;
        }
        let (r, ch, s) = read_wav(&format!("{dir}/{f}"));
        rate = r;
        let f0 = (start * f64::from(r)) as usize;
        let n = (secs * f64::from(r)) as usize;
        if acc.is_empty() {
            acc = vec![0.0; 2 * n];
        }
        for i in 0..n {
            let j = f0 + i;
            if (j + 1) * ch > s.len() {
                break;
            }
            let l = s[j * ch];
            let rr = if ch > 1 { s[j * ch + 1] } else { l };
            acc[2 * i] += l;
            acc[2 * i + 1] += rr;
        }
    }
    let g = 10f64.powf(-14.0 / 20.0) / testpath::mono_power(&acc, 2).sqrt();
    for v in &mut acc {
        *v = (f64::from(*v) * g) as f32;
    }
    (rate, acc)
}

type Mel = Box<dyn Fn(f64) -> Option<f64> + Sync + Send>;

/// The benchmark's male melody (A2..E4) shifted by `st` semitones; `rest` s of rest after
/// every four notes, `br` s of breath between notes, from `start` s on.
fn melody(st: f64, rest: f64, br: f64, start: f64) -> Mel {
    Box::new(move |t: f64| {
        if t < start {
            return None;
        }
        let t = t - start;
        let note_s = 0.75;
        let phrase = 4.0 * (note_s + br) + rest;
        let notes: [f64; 16] = [
            45.0, 47.0, 48.0, 50.0, 52.0, 50.0, 48.0, 47.0, 52.0, 55.0, 57.0, 55.0, 60.0, 59.0,
            57.0, 64.0,
        ];
        let pi = (t / phrase) as usize;
        let tp = t - pi as f64 * phrase;
        let k = (tp / (note_s + br)) as usize;
        if k >= 4 {
            return None;
        }
        let tn = tp - k as f64 * (note_s + br);
        if tn >= note_s {
            return None;
        }
        let base = tv::midi_hz(notes[(pi * 4 + k) % 16] + st);
        Some(if tn > 0.25 {
            tv::vibrato_hz(base, 25.0, 5.5, tn, 0.25)
        } else {
            base
        })
    })
}

/// Scoring truth: `Ok(pitch)` steadily on a note, `Err(true)` in a rest well away from any
/// note, `Err(false)` near an onset or release (not scored).
fn truth(m: &Mel, t: f64) -> Result<f64, bool> {
    match (m(t - 0.03), m(t), m(t + 0.03)) {
        (Some(_), Some(f), Some(_)) => Ok(f),
        (None, None, None) => Err(m(t - 0.08).is_none() && m(t + 0.08).is_none()),
        _ => Err(false),
    }
}

struct Case {
    name: String,
    song: String,
    group: &'static str,
    rate: u32,
    reference: Vec<f32>,
    echo: Vec<f32>,
    voice: Vec<f32>,
    mel: Option<Mel>,
    /// Loopback switched off (or on) at this time, s.
    switch_s: Option<f64>,
}

#[derive(Default, Clone, Copy)]
struct Count {
    voiced: u32,
    right_out: u32,
    right_gated: u32,
    rest: u32,
    rest_shown: u32,
    all: u32,
    shown: u32,
}

impl Count {
    fn pct(a: u32, b: u32) -> f64 {
        100.0 * f64::from(a) / f64::from(b.max(1))
    }
}

fn run(c: &Case) -> String {
    let fs = f64::from(c.rate);
    let n = c.voice.len();
    let secs = n as f64 / fs;
    let mut rng = Rng::new(99);
    let na = 10f64.powf(-78.0 / 20.0) * 3f64.sqrt();
    let noise: Vec<f32> = (0..n).map(|_| (na * rng.bipolar()) as f32).collect();
    let input: Vec<f32> = (0..n).map(|i| c.echo[i] + c.voice[i] + noise[i]).collect();
    let mut ec = EchoCanceller::new(c.rate);
    let lat = ec.latency() as i64;
    let mut tr = Tracker::new(c.rate);
    let hop = tr.hop_len() as i64;
    // Half-width of the span whose gate verdict an event gets: half a hop by default; with
    // JK_CASES_TRUST=window, half the tracker's analysis window, as the app's analysis
    // chain asks it (`practice.rs`, `handle`: the whole window the pitch came from).
    let half = if std::env::var("JK_CASES_TRUST").is_ok_and(|v| v == "window") {
        tr.window_len() as i64 / 2
    } else {
        hop / 2
    };
    let blk = (c.rate / 100) as usize;
    let mut out = vec![0.0f32; n];
    // Per 10 ms block: time, presence, quality.
    let mut timeline: Vec<(f64, Presence, RemovalQuality)> = Vec::new();
    let mut ev: Vec<(f64, Option<f32>, Trust)> = Vec::new();
    // Reasons behind each event's verdict (diagnostics, see `debug_trust_reasons`).
    let mut why: Vec<[u32; 8]> = Vec::new();
    // The canceller's own estimate of the singer's moments its gate takes away, averaged from
    // 15 s (diagnostics, see `debug_doubt`).
    let (mut doubt_sum, mut doubt_n) = (0.0f64, 0u32);
    let trace = std::env::var("JK_ECHO_TRACE").is_ok_and(|v| c.name == v);
    // The true left-over averaged over 10 s (from the first reported left-over on), and
    // the reported left-over minus it, every 0.1 s after 5 s (dB; negative = optimistic).
    let a_true = 1.0 - (-(blk as f64 / fs) / 10.0f64).exp();
    let (mut true_avg, mut true_n) = (0.0f64, 0u32);
    let mut cal: Vec<f64> = Vec::new();
    let (mut q_n, mut q_opt) = (0u32, 0u32);
    let r_voice_of = |r: &jk_pitch::echo::EchoReport| r.voice_dbfs.map(f64::from);
    let r_quality_of = |r: &jk_pitch::echo::EchoReport| r.quality;
    let mut i = 0;
    while i < n {
        let k = blk.min(n - i);
        ec.process(
            &c.reference[2 * i..2 * (i + k)],
            &input[i..i + k],
            &mut out[i..i + k],
        );
        let r = ec.report();
        if (i + k) as f64 / fs >= 15.0
            && let Some(d) = ec.debug_doubt()
        {
            doubt_sum += d;
            doubt_n += 1;
        }
        timeline.push(((i + k) as f64 / fs, r.presence, r.quality));
        if (true_n > 0 || r.residual_dbfs.is_some()) && c.switch_s.is_none() {
            let l = lat as usize;
            let mut p = 0.0f64;
            for j in i.max(l)..i + k {
                let m = j - l;
                p += (f64::from(out[j]) - f64::from(c.voice[m]) - f64::from(noise[m])).powi(2);
            }
            true_n += 1;
            let w = a_true.max(1.0 / f64::from(true_n));
            true_avg += w * (p / k as f64 - true_avg);
            if (i + k) as f64 / fs > 5.0
                && (i + k).is_multiple_of(c.rate as usize / 10)
                && let Some(e) = r.residual_dbfs
            {
                cal.push(f64::from(e) - 10.0 * true_avg.max(1e-30).log10());
                // The verdict the true left-over would give against the reported singer
                // level (25 / 15 dB under it), and whether the reported one is better.
                if let Some(v) = r_voice_of(&r) {
                    let t_db = 10.0 * true_avg.max(1e-30).log10();
                    let q_true = if t_db <= v - 25.0 { 2 } else if t_db <= v - 15.0 { 1 } else { 0 };
                    let q_rep = match r_quality_of(&r) { RemovalQuality::Good => 2, RemovalQuality::Marginal => 1, _ => 0 };
                    q_n += 1;
                    if q_rep > q_true {
                        q_opt += 1;
                    }
                }
            }
        }
        if trace && (i + k).is_multiple_of(c.rate as usize / 10) {
            eprintln!(
                "t {:5.1} {:?} {:?} z {:.1} sup {:?} resid {:?} voice {:?} lost {} relocks {} adapted {}",
                (i + k) as f64 / fs,
                r.presence,
                r.quality,
                r.z,
                r.suppression_db,
                r.residual_dbfs,
                r.voice_dbfs,
                r.lost,
                r.relocks,
                r.adapted
            );
            eprintln!("      {}", ec.debug_line());
        }
        tr.process(&out[i..i + k], |e| {
            let ti = e.time - lat;
            if ti < 0 {
                return;
            }
            ev.push((
                ti as f64 / fs,
                e.hz,
                ec.trust_window(ti - half, ti + half),
            ));
            why.push(ec.debug_trust_reasons(ti - half, ti + half));
        });
        i += k;
    }
    let rep = ec.report();
    // Blocks behind the steady sung frames (from 15 s) the gate took away, by reason.
    let mut lost_why = [0u64; 8];
    if let Some(m) = &c.mel {
        for (k, &(t, hz, w)) in ev.iter().enumerate() {
            if t < 15.0 || matches!(w, Trust::Clean | Trust::Singer) {
                continue;
            }
            if let (Ok(f), Some(h)) = (truth(m, t), hz)
                && tv::cents(f64::from(h), f).abs() < 50.0
            {
                for (a, b) in lost_why.iter_mut().zip(why[k]) {
                    *a += u64::from(b);
                }
            }
        }
    }
    // Frame counts over [a, b).
    let count = |a: f64, b: f64| {
        let mut ct = Count::default();
        for &(t, hz, w) in ev.iter().filter(|e| e.0 >= a && e.0 < b) {
            let trusted = matches!(w, Trust::Clean | Trust::Singer);
            ct.all += 1;
            if trusted && hz.is_some() {
                ct.shown += 1;
            }
            if let Some(m) = &c.mel {
                match truth(m, t) {
                    Ok(f) => {
                        ct.voiced += 1;
                        if let Some(h) = hz
                            && tv::cents(f64::from(h), f).abs() < 50.0
                        {
                            ct.right_out += 1;
                            if trusted {
                                ct.right_gated += 1;
                            }
                        }
                    }
                    Err(true) => {
                        ct.rest += 1;
                        if trusted && hz.is_some() {
                            ct.rest_shown += 1;
                        }
                    }
                    Err(false) => {}
                }
            }
        }
        ct
    };
    // The verdicts: when "present" cleared after the loopback was switched off (or was
    // found after it was switched on), when quality first left "checking".
    let sw = c.switch_s.unwrap_or(0.0);
    let settled = |pred: &dyn Fn(Presence) -> bool| -> Option<f64> {
        let mut first = None;
        for &(t, p, _) in timeline.iter().filter(|x| x.0 >= sw) {
            if pred(p) {
                first.get_or_insert(t);
            } else {
                first = None;
            }
        }
        first.map(|t| t - sw)
    };
    let cleared = settled(&|p| p != Presence::Present);
    let found = settled(&|p| p == Presence::Present);
    let first_verdict = timeline
        .iter()
        .find(|x| {
            matches!(
                x.2,
                RemovalQuality::Good | RemovalQuality::Marginal | RemovalQuality::Unusable
            )
        })
        .map(|x| x.0);
    // True suppression from 15 s on: the echo in the input over what is left of it in the
    // output (output minus the voice and the noise, aligned by the canceller's latency).
    let (mut pe, mut pr) = (0.0f64, 0.0f64);
    for j in (15.0 * fs) as usize + lat as usize..n {
        let k = j - lat as usize;
        let res = f64::from(out[j]) - f64::from(c.voice[k]) - f64::from(noise[k]);
        pe += f64::from(c.echo[k]).powi(2);
        pr += res * res;
    }
    let true_sup = (pe > 0.0).then(|| 10.0 * (pe / pr.max(1e-30)).log10());
    let whole = count(0.0, secs);
    let steady = count(15.0, secs);
    let after = count(sw, (sw + 5.0).min(secs));
    let q = |r: RemovalQuality| format!("{r:?}").to_lowercase();
    let o = |v: Option<f64>| v.map_or("null".to_string(), |x| format!("{x:.2}"));
    cal.sort_by(f64::total_cmp);
    let cal_min = cal.first().copied();
    let cal_p05 = (!cal.is_empty()).then(|| cal[cal.len() / 20]);
    let cal_med = (!cal.is_empty()).then(|| cal[cal.len() / 2]);
    let mut s = String::new();
    let _ = write!(
        s,
        "{{\"song\":\"{}\",\"group\":\"{}\",\"name\":\"{}\",\"presence\":\"{}\",\"quality\":\"{}\",\"relocks\":{},\"lost\":{},\"cleared_after_s\":{},\"found_after_s\":{},\"first_verdict_s\":{},",
        c.song,
        c.group,
        c.name,
        format!("{:?}", rep.presence).to_lowercase(),
        q(rep.quality),
        rep.relocks,
        rep.lost,
        o(cleared),
        o(found),
        o(first_verdict)
    );
    let _ = write!(
        s,
        "\"acc_out_steady\":{:.1},\"acc_gated_steady\":{:.1},\"acc_gated_whole\":{:.1},\"acc_out_after_switch\":{:.1},\"acc_gated_after_switch\":{:.1},\"rest_shown_steady\":{:.2},\"shown_whole\":{:.2},\"shown_after_switch\":{:.2},\"voice_dbfs\":{},\"suppression_db\":{},\"true_suppression_db\":{},\"lost_why\":{:?},\"resid_err_min_db\":{},\"resid_err_p05_db\":{},\"resid_err_med_db\":{},\"q_optimistic_share\":{:.3},\"doubt\":{}}}",
        Count::pct(steady.right_out, steady.voiced),
        Count::pct(steady.right_gated, steady.voiced),
        Count::pct(whole.right_gated, whole.voiced),
        Count::pct(after.right_out, after.voiced),
        Count::pct(after.right_gated, after.voiced),
        Count::pct(steady.rest_shown, steady.rest),
        Count::pct(whole.shown, whole.all),
        Count::pct(after.shown, after.all),
        o(rep.voice_dbfs.map(f64::from)),
        o(rep.suppression_db.map(f64::from)),
        o(true_sup),
        lost_why,
        o(cal_min),
        o(cal_p05),
        o(cal_med),
        f64::from(q_opt) / f64::from(q_n.max(1)),
        o((doubt_n > 0).then(|| 100.0 * doubt_sum / f64::from(doubt_n)))
    );
    s
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let get = |k: &str| {
        args.iter()
            .position(|a| a == k)
            .and_then(|i| args.get(i + 1))
            .cloned()
    };
    let tracks = get("--tracks").expect("--tracks <dir>");
    let songs = get("--songs").unwrap_or_else(|| "ccm-22364".into());
    let out_dir = get("--out").unwrap_or_else(|| ".".into());
    let only = get("--only");
    let threads: usize = get("--threads").and_then(|v| v.parse().ok()).unwrap_or(8);
    std::fs::create_dir_all(&out_dir).expect("output folder");
    let secs = 45.0;
    let mut cases: Vec<Case> = Vec::new();
    for name in songs.split(',') {
        let (rate, music) = song(&format!("{tracks}/{name}"), 30.0, secs);
        let frames = music.len() / 2;
        let fs = f64::from(rate);
        let sing = |m: &Mel, v: Voice| tv::sing(rate, secs, v, m);
        let v18 = Voice {
            vowel: Vowel::Ah,
            level_dbfs: -18.0,
            breath_db: Some(-30.0),
            seed: 5,
        };
        let silent = vec![0.0f32; frames];
        // Loopback switched off (`off`) or on at `at` s: a 10 ms ramp.
        let switch = |e: &mut [f32], at: f64, off: bool| {
            for (i, v) in e.iter_mut().enumerate() {
                let g = ((i as f64 / fs - at) / 0.01).clamp(0.0, 1.0);
                *v *= if off { 1.0 - g } else { g } as f32;
            }
        };
        let echo_of = |p: &ReturnPath, r: &[f32]| testpath::apply(p, r, 2, rate).0;
        // A synthetic guide vocal ("oo", -20 dBFS) mixed into the playback; the singer
        // sings along with it 12 cents sharp and 40 ms late (never the same pitch track:
        // a synthetic voice on exactly the guide's track is a filtered copy of it).
        let guide = sing(
            &melody(0.0, 1.2, 0.15, 0.0),
            Voice {
                vowel: Vowel::Oo,
                level_dbfs: -20.0,
                breath_db: Some(-30.0),
                seed: 77,
            },
        );
        let ref_guide: Vec<f32> = music
            .iter()
            .enumerate()
            .map(|(i, v)| v + guide[i / 2])
            .collect();
        let along = || -> Mel {
            let m = melody(0.0, 1.2, 0.15, 0.0);
            Box::new(move |t: f64| m(t - 0.04).map(|f| f * 2f64.powf(12.0 / 1200.0)))
        };
        let male = || melody(0.0, 1.2, 0.15, 0.0);
        let high = || melody(15.0, 1.2, 0.15, 0.0);
        let cont = || melody(0.0, 0.0, 0.10, 0.0);
        let mut add = |group: &'static str,
                       case: &str,
                       reference: &[f32],
                       echo: Vec<f32>,
                       voice: Vec<f32>,
                       mel: Option<Mel>,
                       switch_s: Option<f64>| {
            if only.as_ref().is_none_or(|o| case.contains(o.as_str())) {
                cases.push(Case {
                    name: case.into(),
                    song: name.into(),
                    group,
                    rate,
                    reference: reference.to_vec(),
                    echo,
                    voice,
                    mel,
                    switch_s,
                });
            }
        };
        // 1. Loopback switched off at 20 s (what the warning tells the owner to do).
        for (label, path, guided, singer) in [
            ("usb0dB_nosinger", ReturnPath::usb(0.0, 0.010), false, false),
            ("usb0dB+guide_nosinger", ReturnPath::usb(0.0, 0.010), true, false),
            ("usb-20dB_nosinger", ReturnPath::usb(-20.0, 0.010), false, false),
            ("usb-40dB_nosinger", ReturnPath::usb(-40.0, 0.010), false, false),
            ("cable0dB_nosinger", ReturnPath::cable(0.0, 0.007, 100.0), false, false),
            ("usb-20dB_singer", ReturnPath::usb(-20.0, 0.010), false, true),
            ("usb-10dB_singer", ReturnPath::usb(-10.0, 0.010), false, true),
            ("usb0dB+guide_singer_along", ReturnPath::usb(0.0, 0.010), true, true),
            ("cable0dB_singer", ReturnPath::cable(0.0, 0.007, 100.0), false, true),
        ] {
            let r: &[f32] = if guided { &ref_guide } else { &music };
            let mut e = echo_of(&path, r);
            switch(&mut e, 20.0, true);
            let (v, m) = if singer {
                if guided {
                    (sing(&along(), v18), Some(along()))
                } else {
                    (sing(&male(), v18), Some(male()))
                }
            } else {
                (silent.clone(), None)
            };
            add("off", &format!("off_at_20s_{label}"), r, e, v, m, Some(20.0));
        }
        // 1b. Switched on at 20 s (found again quickly).
        for (label, path, singer) in [
            ("cable0dB+guide_singer_along", ReturnPath::cable(0.0, 0.007, 100.0), true),
            ("usb-10dB_nosinger", ReturnPath::usb(-10.0, 0.010), false),
        ] {
            let mut e = echo_of(&path, &ref_guide);
            switch(&mut e, 20.0, false);
            let (v, m) = if singer {
                (sing(&along(), v18), Some(along()))
            } else {
                (silent.clone(), None)
            };
            add("on", &format!("on_at_20s_{label}"), &ref_guide, e, v, m, Some(20.0));
        }
        // 2. A high voice (C4..G5) on "ee" and "oo", at several return levels, USB and
        //    cable; "ah" and a male "ee" as controls.
        for (label, vowel, mel) in [
            ("high_ee", Vowel::Ee, high()),
            ("high_oo", Vowel::Oo, high()),
            ("high_ah", Vowel::Ah, high()),
            ("male_ee", Vowel::Ee, male()),
        ] {
            let levels: &[f64] = if label.starts_with("high_e") || label.starts_with("high_o") {
                &[-30.0, -20.0, -10.0, 0.0]
            } else {
                &[-10.0]
            };
            let v = sing(&mel, Voice { vowel, ..v18 });
            for &lv in levels {
                add(
                    "high",
                    &format!("{label}_usb{lv:+}dB"),
                    &music,
                    echo_of(&ReturnPath::usb(lv, 0.010), &music),
                    v.clone(),
                    Some(match label {
                        "male_ee" => male(),
                        _ => high(),
                    }),
                    None,
                );
                if label.starts_with("high_e") || label.starts_with("high_o") {
                    add(
                        "high",
                        &format!("{label}_cable{lv:+}dB"),
                        &music,
                        echo_of(&ReturnPath::cable(lv, 0.007, 100.0), &music),
                        v.clone(),
                        Some(high()),
                        None,
                    );
                }
            }
            let _ = mel;
        }
        add(
            "high",
            "high_ee_nothing_returned",
            &music,
            silent.clone(),
            sing(&high(), Voice { vowel: Vowel::Ee, ..v18 }),
            Some(high()),
            None,
        );
        // 3. Continuous singing (0.1 s breaths, no rests) from the first second.
        for (label, path) in [
            ("usb-30dB", ReturnPath::usb(-30.0, 0.010)),
            ("usb-20dB", ReturnPath::usb(-20.0, 0.010)),
            ("usb-10dB", ReturnPath::usb(-10.0, 0.010)),
            ("usb0dB", ReturnPath::usb(0.0, 0.010)),
            ("cable-10dB", ReturnPath::cable(-10.0, 0.007, 100.0)),
            ("cable0dB", ReturnPath::cable(0.0, 0.007, 100.0)),
        ] {
            add(
                "continuous",
                &format!("continuous_{label}"),
                &music,
                echo_of(&path, &music),
                sing(&cont(), v18),
                Some(cont()),
                None,
            );
        }
        let cont_ee = sing(&cont(), Voice { vowel: Vowel::Ee, ..v18 });
        add(
            "continuous",
            "continuous_ee_usb-10dB",
            &music,
            echo_of(&ReturnPath::usb(-10.0, 0.010), &music),
            cont_ee,
            Some(cont()),
            None,
        );
        let intro = || melody(0.0, 0.0, 0.10, 8.0);
        add(
            "continuous",
            "continuous_after_8s_intro_usb-10dB",
            &music,
            echo_of(&ReturnPath::usb(-10.0, 0.010), &music),
            sing(&intro(), v18),
            Some(intro()),
            None,
        );
        // 4. The echo path changes at 20 s (10 ms ramp): the OS output volume or the mixer's
        //    USB-return level turned down or up (the reference is unchanged), and the app's
        //    master volume turned down 12 dB, which the engine's output tap includes, so the
        //    reference and the return change together ("master"). "path-12dB" is the same
        //    master change as a tap taken before the master volume would see it.
        let scale_after = |x: &mut [f32], ch: usize, at: f64, db: f64| {
            let g1 = 10f64.powf(db / 20.0);
            for (i, v) in x.iter_mut().enumerate() {
                let t = (i / ch) as f64 / fs;
                let a = ((t - at) / 0.01).clamp(0.0, 1.0);
                *v *= (1.0 + a * (g1 - 1.0)) as f32;
            }
        };
        for (label, path, db, master, singer) in [
            ("path-6dB_usb-10dB_nosinger", ReturnPath::usb(-10.0, 0.010), -6.0, false, false),
            ("path+6dB_usb-10dB_nosinger", ReturnPath::usb(-10.0, 0.010), 6.0, false, false),
            ("path-20dB_usb0dB_nosinger", ReturnPath::usb(0.0, 0.010), -20.0, false, false),
            ("path-12dB_usb-10dB_nosinger", ReturnPath::usb(-10.0, 0.010), -12.0, false, false),
            ("master-12dB_usb-10dB_nosinger", ReturnPath::usb(-10.0, 0.010), -12.0, true, false),
            ("path-6dB_usb-10dB_singer", ReturnPath::usb(-10.0, 0.010), -6.0, false, true),
            ("path+6dB_cable-10dB_singer", ReturnPath::cable(-10.0, 0.007, 100.0), 6.0, false, true),
            ("path-12dB_usb-10dB_singer", ReturnPath::usb(-10.0, 0.010), -12.0, false, true),
            ("master-12dB_usb-10dB_singer", ReturnPath::usb(-10.0, 0.010), -12.0, true, true),
        ] {
            let mut r = music.clone();
            if master {
                scale_after(&mut r, 2, 20.0, db);
            }
            let mut e = echo_of(&path, &r);
            if !master {
                scale_after(&mut e, 1, 20.0, db);
            }
            let (v, m) = if singer { (sing(&male(), v18), Some(male())) } else { (silent.clone(), None) };
            add("path", &format!("{label}_at_20s"), &r, e, v, m, Some(20.0));
        }
        // Controls: nobody sings, the music (with the guide vocal) returned and kept.
        add(
            "control",
            "usb0dB+guide_nosinger",
            &ref_guide,
            echo_of(&ReturnPath::usb(0.0, 0.010), &ref_guide),
            silent.clone(),
            None,
            None,
        );
        add(
            "control",
            "cable-10dB+guide_nosinger",
            &ref_guide,
            echo_of(&ReturnPath::cable(-10.0, 0.007, -100.0), &ref_guide),
            silent.clone(),
            None,
            None,
        );
    }
    eprintln!("{} cases on {threads} threads", cases.len());
    let next = std::sync::atomic::AtomicUsize::new(0);
    let results = std::sync::Mutex::new(Vec::<(usize, String)>::new());
    std::thread::scope(|sc| {
        for _ in 0..threads {
            sc.spawn(|| {
                loop {
                    let k = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    let Some(c) = cases.get(k) else { break };
                    let line = run(c);
                    eprintln!("{line}");
                    results.lock().expect("results").push((k, line));
                }
            });
        }
    });
    let mut res = results.into_inner().expect("results");
    res.sort_by_key(|(k, _)| *k);
    let mut s = String::new();
    for (_, l) in &res {
        s.push_str(l);
        s.push('\n');
    }
    std::fs::write(format!("{out_dir}/cases.jsonl"), s).expect("write cases.jsonl");
}

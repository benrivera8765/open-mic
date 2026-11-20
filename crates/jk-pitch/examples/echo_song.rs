// SPDX-License-Identifier: GPL-3.0-or-later
//! Whole-song check of the echo canceller with any singer: a real lead vocal stem (the
//! original singer of a Creative Commons song) or a generated performance written by
//! `jukeoke-cli practice run --write-input`, over the song's own playback returned as a
//! mixer would return it. Complements `echo_eval.rs` (one synthetic male melody over 45 s
//! excerpts) for docs/benchmarks/practice-echo.md: here the truth for every frame is the
//! live tracker on the voice alone, so any voice can be the singer.
//!
//! ```text
//! # A ccMixter track folder (instrumental.wav + backing.wav = what the app plays,
//! # lead.wav = the singer, at its level in the song), or a MUSDB18-HQ song folder with
//! # --musdb (bass + drums + other = what the app plays, vocals = the singer):
//! cargo run --release -p jk-pitch --example echo_song -- --track <dir> \
//!     --returns "none;usb,0,12;cable,-3,25,60;bluetooth,-6,180" --out results.jsonl
//! # A rendered karaoke mix (jukeoke-cli render ... --guide 0) and a generated voice:
//! cargo run --release -p jk-pitch --example echo_song -- --reference mix.wav \
//!     --voice voice.wav --returns "usb,0,12" --out results.jsonl --name feel-perfect
//! ```
//!
//! A return is `kind,level_db,delay_ms[,drift_ppm]` with kind usb | usb_eq | cable |
//! bluetooth, or `none`. Nothing touches a microphone; only numbers are written (one JSON
//! line per return, appended to `--out`). `JK_ECHO_TRACE=<return>` prints the canceller's
//! state every second for that return.

#![allow(clippy::needless_range_loop)] // index loops over aligned signals read plainly

use std::fmt::Write as _;
use std::io::Write as _;

use jk_pitch::echo::testpath::{self, ReturnPath};
use jk_pitch::echo::{EchoCanceller, Presence, RemovalQuality, Trust};
use jk_pitch::tracker::Tracker;
use jk_pitch::tracker::testvoice::Rng;

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

/// Stereo frames `start..start+secs` (mono doubled).
fn stereo(ch: usize, s: &[f32], rate: u32, start: f64, secs: Option<f64>) -> Vec<f32> {
    let frames = s.len() / ch;
    let f0 = ((start * f64::from(rate)) as usize).min(frames);
    let n = secs.map_or(frames - f0, |x| ((x * f64::from(rate)) as usize).min(frames - f0));
    let mut out = vec![0.0f32; 2 * n];
    for i in 0..n {
        let j = (f0 + i) * ch;
        out[2 * i] = s[j];
        out[2 * i + 1] = if ch > 1 { s[j + 1] } else { s[j] };
    }
    out
}

fn db(x: f64) -> f64 {
    10.0 * x.max(1e-30).log10()
}

fn opt(v: Option<f64>) -> String {
    v.filter(|x| x.is_finite()).map_or("null".into(), |x| format!("{x:.3}"))
}

struct Ret {
    label: String,
    path: Option<ReturnPath>,
}

fn parse_return(s: &str) -> Ret {
    let p: Vec<&str> = s.split(',').map(str::trim).collect();
    let num = |i: usize, d: f64| p.get(i).and_then(|v| v.parse::<f64>().ok()).unwrap_or(d);
    let (lv, dl) = (num(1, 0.0), num(2, 10.0) * 1e-3);
    let path = match p[0] {
        "none" => None,
        "usb" => Some(ReturnPath::usb(lv, dl)),
        "usb_eq" => Some(ReturnPath::usb_eq(lv, dl)),
        "cable" => Some(ReturnPath::cable(lv, dl, num(3, 60.0))),
        "bluetooth" => Some(ReturnPath::bluetooth(lv, dl)),
        k => panic!("unknown return kind {k}"),
    };
    Ret { label: s.replace(',', "_"), path }
}

/// The live tracker over a whole signal: (time s, hz, level dBFS) per event.
fn track(rate: u32, x: &[f32]) -> Vec<(f64, Option<f32>, f32)> {
    let mut tr = Tracker::new(rate);
    let mut v = Vec::new();
    let fs = f64::from(rate);
    tr.process(x, |e| v.push((e.time as f64 / fs, e.hz, e.level_dbfs)));
    v
}

fn run(name: &str, rate: u32, reference: &[f32], voice: &[f32], clean: &[(f64, Option<f32>, f32)], r: &Ret) -> String {
    let fs = f64::from(rate);
    let n = voice.len();
    let secs = n as f64 / fs;
    let (echo, _) = match &r.path {
        Some(p) => testpath::apply(p, reference, 2, rate),
        None => (vec![0.0f32; n], vec![0.0; n]),
    };
    let mut rng = Rng::new(99);
    let na = 10f64.powf(-78.0 / 20.0) * 3f64.sqrt();
    let noise: Vec<f32> = (0..n).map(|_| (na * rng.bipolar()) as f32).collect();
    let input: Vec<f32> = (0..n).map(|i| echo[i] + voice[i] + noise[i]).collect();
    let mut ec = EchoCanceller::new(rate);
    let lat = ec.latency() as i64;
    let mut tr = Tracker::new(rate);
    let half = tr.window_len() as i64 / 2;
    let blk = (rate / 100) as usize;
    let mut out = vec![0.0f32; n];
    let mut ev: Vec<(f64, Option<f32>, Trust)> = Vec::new();
    let mut why: Vec<[u32; 8]> = Vec::new();
    let trace = std::env::var("JK_ECHO_TRACE").is_ok_and(|v| v == r.label);
    let (mut first_present, mut present_s) = (None::<f64>, 0.0f64);
    // Seconds while present per quality: checking, good, marginal, unusable.
    let mut q_s = [0.0f64; 4];
    let dt = blk as f64 / fs;
    // The true left-over averaged like the canceller's coherence measurement (10 s), and
    // how the reported left-over compares with it every 0.1 s once both exist.
    let a_true = 1.0 - (-dt / 10.0f64).exp();
    let (mut true_avg, mut true_n) = (0.0f64, 0u32);
    let mut cal: Vec<f64> = Vec::new();
    let (mut q_n, mut q_opt) = (0u32, 0u32);
    let r_voice_of = |r: &jk_pitch::echo::EchoReport| r.voice_dbfs.map(f64::from);
    let r_quality_of = |r: &jk_pitch::echo::EchoReport| r.quality;
    let mut i = 0;
    while i < n {
        let k = blk.min(n - i);
        ec.process(&reference[2 * i..2 * (i + k)], &input[i..i + k], &mut out[i..i + k]);
        let rep = ec.report();
        let t = (i + k) as f64 / fs;
        // (Only from the first reported left-over on: before it the filter is still
        // learning, and the canceller's measurement starts with the filter.)
        if r.path.is_some() && (true_n > 0 || rep.residual_dbfs.is_some()) {
            let l = lat as usize;
            let mut p = 0.0f64;
            for j in (i.max(l))..i + k {
                let m = j - l;
                p += (f64::from(out[j]) - f64::from(voice[m]) - f64::from(noise[m])).powi(2);
            }
            let p = p / k as f64;
            true_n += 1;
            let w = a_true.max(1.0 / f64::from(true_n));
            true_avg += w * (p - true_avg);
            if t > 5.0
                && (i + k).is_multiple_of(rate as usize / 10)
                && let Some(e) = rep.residual_dbfs
            {
                cal.push(f64::from(e) - db(true_avg));
                // The verdict the true left-over would give against the reported singer
                // level (25 / 15 dB under it), and whether the reported one is better.
                if let Some(v) = r_voice_of(&rep) {
                    let t_db = 10.0 * true_avg.max(1e-30).log10();
                    let q_true = if t_db <= v - 25.0 { 2 } else if t_db <= v - 15.0 { 1 } else { 0 };
                    let q_rep = match r_quality_of(&rep) { RemovalQuality::Good => 2, RemovalQuality::Marginal => 1, _ => 0 };
                    q_n += 1;
                    if q_rep > q_true {
                        q_opt += 1;
                    }
                }
            }
        }
        if rep.presence == Presence::Present {
            first_present.get_or_insert(t);
            present_s += dt;
            let qi = match rep.quality {
                RemovalQuality::Good => 1,
                RemovalQuality::Marginal => 2,
                RemovalQuality::Unusable => 3,
                _ => 0,
            };
            q_s[qi] += dt;
        }
        if trace && (i + k).is_multiple_of(rate as usize) {
            // True residual over the last second (output minus voice and noise).
            let (mut a, mut b, mut v) = (0.0f64, 0.0f64, 0.0f64);
            let l = lat as usize;
            for j in (i + k).saturating_sub(rate as usize).max(l)..i + k {
                let m = j - l;
                a += f64::from(echo[m]).powi(2);
                b += (f64::from(out[j]) - f64::from(voice[m]) - f64::from(noise[m])).powi(2);
                v += f64::from(voice[m]).powi(2);
            }
            eprintln!(
                "t {t:6.1} {:?} {:?} {:?} z {:.1} erle {:5.1} est {:?} resid true {:5.1} est {:?} voice true {:5.1} est {:?} lvl {:?} dly {:?} relocks {} lost {}",
                rep.presence,
                rep.quality,
                rep.stability,
                rep.z,
                db(a / b.max(1e-30)),
                rep.suppression_db,
                db(b / fs),
                rep.residual_dbfs,
                db(v / fs),
                rep.voice_dbfs,
                rep.level_db,
                rep.delay_ms,
                rep.relocks,
                rep.lost
            );
            eprintln!("        true10 {:.1} | {}", db(true_avg), ec.debug_line());
        }
        tr.process(&out[i..i + k], |e| {
            let ti = e.time - lat;
            if ti < 0 {
                return;
            }
            ev.push((ti as f64 / fs, e.hz, ec.trust_window(ti - half, ti + half)));
            why.push(ec.debug_trust_reasons(ti - half, ti + half));
        });
        i += k;
    }
    let rep = ec.report();
    // Frames: the clean tracker (voice alone) is the truth. Events are at the same times
    // (same hop, same start) on both, so match by index after aligning the first time.
    let (mut sung, mut kept, mut kept_raw, mut wrong_shown, mut silent, mut false_shown) = (0u32, 0u32, 0u32, 0u32, 0u32, 0u32);
    let mut trust_n = [0u32; 4];
    // Diagnostics (JK_SONG_HIST=1): sung frames and those the gate took away, by the
    // voice-to-echo ratio at the input and the voice-to-left-over ratio at the output over
    // the tracker window.
    let hist = std::env::var("JK_SONG_HIST").is_ok();
    let cum = |x: &dyn Fn(usize) -> f64| -> Vec<f64> {
        let mut c = vec![0.0f64; n + 1];
        for i in 0..n {
            c[i + 1] = c[i] + x(i);
        }
        c
    };
    let (cv, ce, cr) = if hist {
        let l = lat as usize;
        (
            cum(&|i| f64::from(voice[i]).powi(2)),
            cum(&|i| f64::from(echo[i]).powi(2)),
            cum(&|i| {
                let j = i + l;
                if j < n {
                    (f64::from(out[j]) - f64::from(voice[i]) - f64::from(noise[i])).powi(2)
                } else {
                    0.0
                }
            }),
        )
    } else {
        (Vec::new(), Vec::new(), Vec::new())
    };
    let edges = [-10.0, -5.0, 0.0, 5.0, 10.0, 20.0, 30.0];
    let mut h_ver = [[0u32; 2]; 8];
    let mut h_vrr = [[0u32; 2]; 8];
    // Reasons of the blocks behind the sung frames the gate took away.
    let mut lost_why = [0u64; 8];
    // Sung frames the gate took away (right pitch after removal, not shown), by verdict.
    let mut lost_by = [0u32; 4];
    let from = 0.0;
    let hop_s = tr.hop_len() as f64 / fs;
    let mut j = 0usize;
    for (ei, &(t, hz, w)) in ev.iter().enumerate() {
        // The clean event nearest in time (the output's events are `latency` samples
        // later, which is not a whole number of hops).
        while j + 1 < clean.len() && (clean[j + 1].0 - t).abs() <= (clean[j].0 - t).abs() {
            j += 1;
        }
        if j >= clean.len() || (clean[j].0 - t).abs() > 0.5 * hop_s + 1e-6 || t < from {
            continue;
        }
        trust_n[w as usize] += 1;
        let shown = matches!(w, Trust::Clean | Trust::Singer);
        let (_, ch, cl) = clean[j];
        match ch {
            Some(c) => {
                sung += 1;
                if let Some(h) = hz {
                    let cents = 1200.0 * (f64::from(h) / f64::from(c)).log2();
                    if cents.abs() < 50.0 && hist {
                        let c = (t * fs) as usize;
                        let (a, b2) = (c.saturating_sub(half as usize), (c + half as usize).min(n));
                        let v = cv[b2] - cv[a];
                        let ver = db(v / (ce[b2] - ce[a]).max(1e-30));
                        let vrr = db(v / (cr[b2] - cr[a]).max(1e-30));
                        let bin = |x: f64| edges.iter().filter(|&&e| x >= e).count();
                        let lostf = usize::from(!shown);
                        h_ver[bin(ver)][0] += 1;
                        h_ver[bin(ver)][1] += lostf as u32;
                        h_vrr[bin(vrr)][0] += 1;
                        h_vrr[bin(vrr)][1] += lostf as u32;
                    }
                    if cents.abs() < 50.0 {
                        kept_raw += 1;
                        if shown {
                            kept += 1;
                        } else {
                            lost_by[w as usize] += 1;
                            for (a, b2) in lost_why.iter_mut().zip(why[ei]) {
                                *a += u64::from(b2);
                            }
                        }
                    } else if shown {
                        wrong_shown += 1;
                    }
                }
            }
            None => {
                if cl < -60.0 {
                    silent += 1;
                    if shown && hz.is_some() {
                        false_shown += 1;
                    }
                }
            }
        }
    }
    // True suppression and levels over the last two thirds.
    let from_i = (secs / 3.0 * fs) as usize + lat as usize;
    let (mut pe, mut pr, mut pv) = (0.0f64, 0.0f64, 0.0f64);
    for jj in from_i..n {
        let m = jj - lat as usize;
        let res = f64::from(out[jj]) - f64::from(voice[m]) - f64::from(noise[m]);
        pe += f64::from(echo[m]).powi(2);
        pr += res * res;
        pv += f64::from(voice[m]).powi(2);
    }
    let cnt = (n - from_i.min(n)).max(1) as f64;
    let pct = |a: u32, b: u32| 100.0 * f64::from(a) / f64::from(b.max(1));
    if hist {
        let fmt = |h: &[[u32; 2]; 8]| {
            let mut o = String::new();
            for (i, x) in h.iter().enumerate() {
                let lo = if i == 0 { "-inf".to_string() } else { format!("{}", edges[i - 1]) };
                let _ = write!(o, " [{lo}: {}/{} {:.0}%]", x[1], x[0], pct(x[1], x[0]));
            }
            o
        };
        eprintln!("{} {}: lost/sung by voice-to-echo dB:{}", name, r.label, fmt(&h_ver));
        eprintln!("{} {}: lost/sung by voice-to-left-over dB:{}", name, r.label, fmt(&h_vrr));
        eprintln!(
            "{} {}: blocks behind lost frames: other {} explained {} under {} held {} doubted {} singer-level {} singer-spectral {}",
            name, r.label, lost_why[0], lost_why[1], lost_why[2], lost_why[3], lost_why[4], lost_why[5], lost_why[6]
        );
    }
    cal.sort_by(f64::total_cmp);
    let cal_med = (!cal.is_empty()).then(|| cal[cal.len() / 2]);
    let cal_opt3 = (!cal.is_empty()).then(|| cal.iter().filter(|&&e| e < -3.0).count() as f64 / cal.len() as f64);
    let cal_min = cal.first().copied();
    let mut s = String::new();
    let _ = write!(
        s,
        "{{\"name\":\"{name}\",\"return\":\"{}\",\"secs\":{secs:.1},\"presence\":\"{}\",\"found_s\":{},\"present_s\":{present_s:.1},\"checking_s\":{:.1},\"good_s\":{:.1},\"marginal_s\":{:.1},\"unusable_s\":{:.1},\"unusable_share\":{:.3},\"quality\":\"{}\",\"stability\":\"{}\",\"relocks\":{},\"lost\":{},",
        r.label,
        format!("{:?}", rep.presence).to_lowercase(),
        opt(first_present),
        q_s[0],
        q_s[1],
        q_s[2],
        q_s[3],
        q_s[3] / secs,
        format!("{:?}", rep.quality).to_lowercase(),
        format!("{:?}", rep.stability).to_lowercase(),
        rep.relocks,
        rep.lost
    );
    let _ = write!(
        s,
        "\"level_db\":{},\"delay_ms\":{},\"true_suppression_db\":{},\"est_suppression_db\":{},\"true_residual_dbfs\":{},\"est_residual_dbfs\":{},\"true_voice_dbfs\":{},\"est_voice_dbfs\":{},",
        opt(rep.level_db.map(f64::from)),
        opt(rep.delay_ms),
        opt((pe > 0.0).then(|| db(pe / pr.max(1e-30)))),
        opt(rep.suppression_db.map(f64::from)),
        opt((pe > 0.0).then(|| db(pr / cnt))),
        opt(rep.residual_dbfs.map(f64::from)),
        opt(Some(db(pv / cnt))),
        opt(rep.voice_dbfs.map(f64::from))
    );
    let _ = write!(
        s,
        "\"resid_err_med_db\":{},\"resid_err_min_db\":{},\"resid_optimistic_3db_share\":{},\"q_optimistic_share\":{:.3},\"lost_music_pct\":{:.2},\"lost_unknown_pct\":{:.2},\"sung_frames\":{sung},\"kept_pct\":{:.2},\"kept_out_pct\":{:.2},\"wrong_shown_pct\":{:.2},\"silent_frames\":{silent},\"false_shown_pct\":{:.3},\"trust_clean\":{},\"trust_singer\":{},\"trust_music\":{},\"trust_unknown\":{}}}",
        opt(cal_med),
        opt(cal_min),
        opt(cal_opt3),
        f64::from(q_opt) / f64::from(q_n.max(1)),
        pct(lost_by[Trust::Music as usize], sung),
        pct(lost_by[Trust::Unknown as usize], sung),
        pct(kept, sung),
        pct(kept_raw, sung),
        pct(wrong_shown, sung),
        pct(false_shown, silent),
        trust_n[0],
        trust_n[1],
        trust_n[2],
        trust_n[3]
    );
    s
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let get = |k: &str| args.iter().position(|a| a == k).and_then(|i| args.get(i + 1)).cloned();
    let start: f64 = get("--start").and_then(|v| v.parse().ok()).unwrap_or(0.0);
    let secs: Option<f64> = get("--secs").and_then(|v| v.parse().ok());
    let voice_db: f64 = get("--voice-db").and_then(|v| v.parse().ok()).unwrap_or(0.0);
    let threads: usize = get("--threads").and_then(|v| v.parse().ok()).unwrap_or(8);
    // A ccMixter track folder (instrumental + backing = the playback, lead = the singer) or a
    // MUSDB18-HQ song folder (bass + drums + other = the playback, vocals = the singer).
    let stems = get("--track").map(|d| (d, ["instrumental.wav", "backing.wav", "", "lead.wav"])).or_else(|| {
        get("--musdb").map(|d| (d, ["bass.wav", "drums.wav", "other.wav", "vocals.wav"]))
    });
    let (rate, reference, voice, name) = if let Some((dir, files)) = stems {
        // (Some ccMixter folders have no instrumental stem: their backing stem is the whole
        // accompaniment.)
        let first = if std::path::Path::new(&format!("{dir}/{}", files[0])).exists() { 0 } else { 1 };
        let (rate, mut music) = {
            let (r, ch, s) = read_wav(&format!("{dir}/{}", files[first]));
            (r, stereo(ch, &s, r, start, secs))
        };
        for f in files[first + 1..3].iter().filter(|f| !f.is_empty()) {
            let (_, ch, s) = read_wav(&format!("{dir}/{f}"));
            for (m, b) in music.iter_mut().zip(stereo(ch, &s, rate, start, secs)) {
                *m += b;
            }
        }
        let (_, ch, s) = read_wav(&format!("{dir}/{}", files[3]));
        let lead = stereo(ch, &s, rate, start, secs);
        // What the app plays at -14 dBFS mono RMS; the singer at its level in the song.
        let g = 10f64.powf(-14.0 / 20.0) / testpath::mono_power(&music, 2).sqrt();
        let gv = g * 10f64.powf(voice_db / 20.0);
        for v in music.iter_mut() {
            *v = (f64::from(*v) * g) as f32;
        }
        let n = music.len() / 2;
        let voice: Vec<f32> = (0..n.min(lead.len() / 2))
            .map(|i| (0.5 * (f64::from(lead[2 * i]) + f64::from(lead[2 * i + 1])) * gv) as f32)
            .collect();
        let name = std::path::Path::new(&dir).file_name().map_or("track".into(), |s| s.to_string_lossy().into_owned());
        (rate, music[..2 * voice.len()].to_vec(), voice, name)
    } else {
        let rpath = get("--reference").expect("--track DIR or --reference WAV --voice WAV");
        let vpath = get("--voice").expect("--voice WAV");
        let (rate, ch, s) = read_wav(&rpath);
        let reference = stereo(ch, &s, rate, start, secs);
        let (vr, vch, vs) = read_wav(&vpath);
        assert_eq!(vr, rate, "voice and reference rates differ");
        let gv = 10f64.powf(voice_db / 20.0);
        let n = (reference.len() / 2).min(vs.len() / vch);
        let f0 = (start * f64::from(rate)) as usize;
        let voice: Vec<f32> = (0..n).map(|i| vs.get((f0 + i) * vch).map_or(0.0, |&v| (f64::from(v) * gv) as f32)).collect();
        (rate, reference[..2 * n].to_vec(), voice, get("--name").unwrap_or_else(|| "song".into()))
    };
    let returns: Vec<Ret> = get("--returns").unwrap_or_else(|| "none".into()).split(';').map(parse_return).collect();
    let out = get("--out").expect("--out results.jsonl");
    let clean = track(rate, &voice);
    eprintln!("{name}: {:.1} s at {rate} Hz, {} returns", voice.len() as f64 / f64::from(rate), returns.len());
    let next = std::sync::atomic::AtomicUsize::new(0);
    let results = std::sync::Mutex::new(Vec::<(usize, String)>::new());
    std::thread::scope(|sc| {
        for _ in 0..threads.min(returns.len()).max(1) {
            sc.spawn(|| {
                loop {
                    let k = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    let Some(r) = returns.get(k) else { break };
                    let line = run(&name, rate, &reference, &voice, &clean, r);
                    eprintln!("{line}");
                    results.lock().expect("results").push((k, line));
                }
            });
        }
    });
    let mut res = results.into_inner().expect("results");
    res.sort_by_key(|(k, _)| *k);
    let mut f = std::fs::OpenOptions::new().create(true).append(true).open(&out).expect("open --out");
    for (_, l) in &res {
        writeln!(f, "{l}").expect("write");
    }
}

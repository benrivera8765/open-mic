// SPDX-License-Identifier: GPL-3.0-or-later
//! Benchmark of the echo canceller (jk_pitch::echo) for docs/benchmarks/practice-echo.md:
//! the app's own playback returned into the input as a mixer would return it (USB
//! loopback, a cable into AUX-IN, Bluetooth), with and without a synthetic singer, with and
//! without the guide vocal in the playback.
//!
//! ```text
//! cargo run --release -p jk-pitch --example echo_eval -- \
//!     --tracks <dir with ccMixter track folders> --out <results dir> [--quick] [--cpu]
//! ```
//!
//! Each ccMixter track folder holds instrumental.wav, backing.wav and lead.wav (Creative
//! Commons, evaluation only, never committed). The backing track (the reference) is
//! instrumental + backing vocals; the guide vocal is the lead part. Nothing here touches a
//! microphone; every signal is generated or read from those files, and only numbers are
//! written (one JSON line per scenario).

#![allow(clippy::needless_range_loop)] // index loops over aligned signals read plainly

use std::fmt::Write as _;
use std::time::Instant;

use jk_pitch::echo::testpath::{self, ReturnPath};
use jk_pitch::echo::{DelayStability, EchoCanceller, Presence, RemovalQuality, Trust};
use jk_pitch::tracker::Tracker;
use jk_pitch::tracker::testvoice::{self as tv, Rng, Voice, Vowel};

// ---------------------------------------------------------------------------------------
// Audio files.

struct Wav {
    rate: u32,
    channels: usize,
    samples: Vec<f32>,
}

#[allow(clippy::chunks_exact_to_as_chunks)] // byte chunks read plainly in a test tool
fn read_wav(path: &str) -> Result<Wav, String> {
    let b = std::fs::read(path).map_err(|e| format!("{path}: {e}"))?;
    if b.len() < 12 || &b[0..4] != b"RIFF" || &b[8..12] != b"WAVE" {
        return Err(format!("{path}: not a RIFF/WAVE file"));
    }
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
            if fmt == 0xFFFE && len >= 26 {
                fmt = u16le(body + 24);
            }
        } else if id == b"data" {
            let end = (body + len).min(b.len());
            let d = &b[body..end];
            let samples: Vec<f32> = match (fmt, bits) {
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
                _ => {
                    return Err(format!(
                        "{path}: unsupported WAV format {fmt} / {bits} bits"
                    ));
                }
            };
            if ch == 0 || rate == 0 {
                return Err(format!("{path}: data before fmt"));
            }
            return Ok(Wav {
                rate,
                channels: ch,
                samples,
            });
        }
        pos = body + len + (len & 1);
    }
    Err(format!("{path}: no data chunk"))
}

/// Stereo interleaved excerpt `start..start + secs` of a file (mono files are doubled).
fn excerpt(w: &Wav, start: f64, secs: f64) -> Vec<f32> {
    let f0 = (start * f64::from(w.rate)) as usize;
    let n = (secs * f64::from(w.rate)) as usize;
    let frames = w.samples.len() / w.channels;
    let mut out = vec![0.0f32; 2 * n];
    for i in 0..n {
        let j = f0 + i;
        if j >= frames {
            break;
        }
        let l = w.samples[j * w.channels];
        let r = if w.channels > 1 {
            w.samples[j * w.channels + 1]
        } else {
            l
        };
        out[2 * i] = l;
        out[2 * i + 1] = r;
    }
    out
}

// ---------------------------------------------------------------------------------------
// The synthetic singer.

/// Melody of the synthetic singer (a male range, A2 to E4): phrases of four notes of
/// 0.75 s with 0.15 s breaths between them, then 1.2 s of rest; vibrato of +-25 cents at
/// 5.5 Hz from 0.25 s into each note.
fn melody(t: f64) -> Option<f64> {
    let phrase = 4.0 * 0.9 + 1.2;
    let notes: [f64; 16] = [
        45.0, 47.0, 48.0, 50.0, 52.0, 50.0, 48.0, 47.0, 52.0, 55.0, 57.0, 55.0, 60.0, 59.0, 57.0,
        64.0,
    ];
    let pi = (t / phrase) as usize;
    let tp = t - pi as f64 * phrase;
    let k = (tp / 0.9) as usize;
    if k >= 4 {
        return None;
    }
    let tn = tp - k as f64 * 0.9;
    if tn >= 0.75 {
        return None;
    }
    let base = tv::midi_hz(notes[(pi * 4 + k) % notes.len()]);
    Some(if tn > 0.25 {
        tv::vibrato_hz(base, 25.0, 5.5, tn, 0.25)
    } else {
        base
    })
}

/// Truth for frame scoring: the melody's pitch at `t` when the singer is steadily on a note
/// there (the tracker window reaches 25 ms either side), `Err(true)` in a rest well away
/// from any note, `Err(false)` near an onset or release (not scored).
fn truth(t: f64) -> Result<f64, bool> {
    let c = melody(t);
    let a = melody(t - 0.03);
    let b = melody(t + 0.03);
    match (a, c, b) {
        (Some(_), Some(f), Some(_)) => Ok(f),
        (None, None, None) => Err(melody(t - 0.08).is_none() && melody(t + 0.08).is_none()),
        _ => Err(false),
    }
}

// ---------------------------------------------------------------------------------------
// Scenarios.

#[derive(Clone, Debug)]
struct Scenario {
    name: String,
    track: String,
    /// "none" (nothing returned), "usb", "usb_eq", "cable", "bluetooth".
    kind: &'static str,
    level_db: f64,
    delay_ms: f64,
    drift_ppm: f64,
    singer: bool,
    guide: bool,
    /// Silent stretch in the music (s).
    gap: Option<(f64, f64)>,
}

fn path_for(s: &Scenario) -> Option<ReturnPath> {
    let d = s.delay_ms * 1e-3;
    match s.kind {
        "usb" => Some(ReturnPath::usb(s.level_db, d)),
        "usb_eq" => Some(ReturnPath::usb_eq(s.level_db, d)),
        "cable" => Some(ReturnPath::cable(s.level_db, d, s.drift_ppm)),
        "bluetooth" => Some(ReturnPath::bluetooth(s.level_db, d)),
        _ => None,
    }
}

/// Tracker frame counts.
#[derive(Default, Clone, Copy)]
struct Frames {
    /// Steady sung frames, of them with the right pitch (within 50 cents).
    voiced: u32,
    right: u32,
    /// Rest frames, of them with a pitch reported.
    rest: u32,
    rest_pitch: u32,
    /// Every frame, of them with a pitch reported (for runs without a singer).
    all: u32,
    all_pitch: u32,
}

impl Frames {
    fn add(&mut self, t: f64, hz: Option<f32>, singer: bool) {
        self.all += 1;
        if hz.is_some() {
            self.all_pitch += 1;
        }
        if !singer {
            return;
        }
        match truth(t) {
            Ok(f) => {
                self.voiced += 1;
                if let Some(h) = hz
                    && tv::cents(f64::from(h), f).abs() < 50.0
                {
                    self.right += 1;
                }
            }
            Err(true) => {
                self.rest += 1;
                if hz.is_some() {
                    self.rest_pitch += 1;
                }
            }
            Err(false) => {}
        }
    }
    fn acc(&self) -> f64 {
        f64::from(self.right) / f64::from(self.voiced.max(1))
    }
    fn rest_rate(&self) -> f64 {
        f64::from(self.rest_pitch) / f64::from(self.rest.max(1))
    }
    fn pitch_rate(&self) -> f64 {
        f64::from(self.all_pitch) / f64::from(self.all.max(1))
    }
}

/// Runs a tracker over a whole signal and scores it (frames from `from_s` on).
fn track_all(rate: u32, x: &[f32], singer: bool, from_s: f64) -> Frames {
    let mut tr = Tracker::new(rate);
    let mut f = Frames::default();
    let fs = f64::from(rate);
    tr.process(x, |e| {
        let t = e.time as f64 / fs;
        if t >= from_s {
            f.add(t, e.hz, singer);
        }
    });
    f
}

struct Outcome {
    json: String,
    line: String,
}

fn db(x: f64) -> f64 {
    10.0 * x.max(1e-30).log10()
}

fn opt(v: Option<f64>) -> String {
    v.map_or("null".into(), |x| {
        if x.is_finite() {
            format!("{x:.4}")
        } else {
            "null".into()
        }
    })
}

/// Runs one scenario on `music` (stereo, interleaved) and `lead` (stereo guide vocal).
fn run(s: &Scenario, rate: u32, music: &[f32], lead: &[f32]) -> Outcome {
    let fs = f64::from(rate);
    let frames = music.len() / 2;
    let secs = frames as f64 / fs;
    // The reference: the backing track, the guide vocal on top when it is on; silent in the
    // break when there is one.
    let mut reference: Vec<f32> = music.to_vec();
    if s.guide {
        for (r, g) in reference.iter_mut().zip(lead) {
            *r += g;
        }
    }
    if let Some((a, b)) = s.gap {
        for i in (a * fs) as usize..((b * fs) as usize).min(frames) {
            reference[2 * i] = 0.0;
            reference[2 * i + 1] = 0.0;
        }
    }
    let ref_pow = testpath::mono_power(&reference, 2);
    // What comes back, and its truth.
    let (echo, delays) = match path_for(s) {
        Some(p) => testpath::apply(&p, &reference, 2, rate),
        None => (vec![0.0f32; frames], vec![0.0; frames]),
    };
    let true_level = db(testpath::power(&echo) / ref_pow);
    let true_delay_end = delays[frames - 1] * 1e3;
    let true_delay_mean = delays.iter().sum::<f64>() / frames as f64 * 1e3;
    // The singer, and the microphone's own noise floor (-78 dBFS).
    let voice = if s.singer {
        tv::sing(
            rate,
            secs,
            Voice {
                vowel: Vowel::Ah,
                level_dbfs: -18.0,
                breath_db: Some(-30.0),
                seed: 5,
            },
            melody,
        )
    } else {
        vec![0.0f32; frames]
    };
    let mut rng = Rng::new(99);
    let na = 10f64.powf(-78.0 / 20.0) * 3f64.sqrt();
    let input: Vec<f32> = (0..frames)
        .map(|i| echo[i] + voice[i] + (na * rng.bipolar()) as f32)
        .collect();

    // The canceller, with the live tracker on its output and the trust verdict asked for
    // every pitch event as it happens.
    let mut ec = EchoCanceller::new(rate);
    let lat = ec.latency() as i64;
    let mut tr = Tracker::new(rate);
    let hop = tr.hop_len() as i64;
    let blk = (rate / 100) as usize;
    let mut out = vec![0.0f32; frames];
    let mut first_present: Option<f64> = None;
    let mut settled_present: Option<f64> = None;
    let mut first_absent: Option<f64> = None;
    let mut first_quality: Option<f64> = None;
    let mut out_all = Frames::default();
    let mut out_trusted = Frames::default();
    let mut out_steady_all = Frames::default();
    let mut out_steady_trusted = Frames::default();
    let mut trust_counts = [0u32; 4];
    // Times (s) of pitch frames the app would show with nobody singing (the whole run
    // without a singer, the rests between phrases with one).
    let mut false_times: Vec<f64> = Vec::new();
    let mut proc_s = 0.0f64;
    let steady_from = 15.0f64.min(secs * 0.5);
    let trace = std::env::var("JK_ECHO_TRACE").is_ok_and(|v| s.name == v);
    let mut i = 0;
    while i < frames {
        let n = blk.min(frames - i);
        let t0 = Instant::now();
        ec.process(
            &reference[2 * i..2 * (i + n)],
            &input[i..i + n],
            &mut out[i..i + n],
        );
        proc_s += t0.elapsed().as_secs_f64();
        let r = ec.report();
        let t = (i + n) as f64 / fs;
        match r.presence {
            Presence::Present => {
                first_present.get_or_insert(t);
                settled_present.get_or_insert(t);
            }
            Presence::Absent => {
                first_absent.get_or_insert(t);
                settled_present = None;
            }
            Presence::Searching => settled_present = None,
        }
        if matches!(
            r.quality,
            RemovalQuality::Good | RemovalQuality::Marginal | RemovalQuality::Unusable
        ) {
            first_quality.get_or_insert(t);
        }
        if trace && (i + n).is_multiple_of(rate as usize) {
            // Per-second diagnostics: true suppression over the last second.
            let (mut a, mut b) = (0.0f64, 0.0f64);
            let l = lat as usize;
            for j in (i + n).saturating_sub(rate as usize).max(l)..i + n {
                let k = j - l;
                a += f64::from(echo[k]).powi(2);
                b += (f64::from(out[j]) - f64::from(voice[k])).powi(2);
            }
            eprintln!(
                "t {t:5.1} erle {:6.1} est {:?} q {:?} pres {:?} relocks {} upd {} leak {:.4} dly {:?} drift {:?} stab {:?}",
                db(a / b.max(1e-30)),
                r.suppression_db,
                r.quality,
                r.presence,
                r.relocks,
                ec.filter_updates(),
                ec.filter_leak(),
                r.delay_ms,
                r.drift_ppm,
                r.stability
            );
            eprintln!("      detfit {:?} z {:.1}", ec.detector_fit(), r.z);
        }
        tr.process(&out[i..i + n], |e| {
            let ti = e.time - lat;
            if ti < 0 {
                return;
            }
            let tt = ti as f64 / fs;
            let w = ec.trust_window(ti - hop / 2, ti + hop / 2);
            trust_counts[w as usize] += 1;
            let trusted = matches!(w, Trust::Clean | Trust::Singer);
            let resting = !s.singer || truth(tt) == Err(true);
            if trusted && e.hz.is_some() && resting && false_times.len() < 400 {
                false_times.push((tt * 100.0).round() / 100.0);
            }
            out_all.add(tt, e.hz, s.singer);
            out_trusted.add(tt, if trusted { e.hz } else { None }, s.singer);
            if tt >= steady_from {
                out_steady_all.add(tt, e.hz, s.singer);
                out_steady_trusted.add(tt, if trusted { e.hz } else { None }, s.singer);
            }
        });
        i += n;
    }
    let rep = ec.report();
    // True suppression over the steady part: echo in, echo left (output minus the singer
    // and the noise, aligned by the canceller's latency).
    let (mut pe, mut pr) = (0.0f64, 0.0f64);
    let mut rng2 = Rng::new(99);
    let mut noise = vec![0.0f32; frames];
    for v in noise.iter_mut() {
        *v = (na * rng2.bipolar()) as f32;
    }
    let from = (steady_from * fs) as usize;
    for j in from + lat as usize..frames {
        let k = j - lat as usize;
        let res = f64::from(out[j]) - f64::from(voice[k]) - f64::from(noise[k]);
        pe += f64::from(echo[k]).powi(2);
        pr += res * res;
    }
    let erle = if pe > 0.0 {
        Some(db(pe / pr.max(1e-30)))
    } else {
        None
    };
    let n_res = (frames - from - lat as usize).max(1) as f64;
    let residual_dbfs = (pe > 0.0).then(|| db(pr / n_res));
    // The tracker on the singer alone and on the raw input (no removal), for comparison.
    let clean = track_all(rate, &voice, s.singer, 0.0);
    let raw = track_all(rate, &input, s.singer, 0.0);
    let raw_steady = track_all(rate, &input, s.singer, steady_from);
    let stab = |d: DelayStability| format!("{d:?}").to_lowercase();
    let q = format!("{:?}", rep.quality).to_lowercase();
    let pres = format!("{:?}", rep.presence).to_lowercase();
    let mut json = String::new();
    let _ = write!(
        json,
        "{{\"name\":\"{}\",\"track\":\"{}\",\"rate\":{},\"secs\":{:.1},\"kind\":\"{}\",\"return_level_db\":{},\"return_delay_ms\":{},\"return_drift_ppm\":{},\"singer\":{},\"guide\":{},\"gap\":{},",
        s.name,
        s.track,
        rate,
        secs,
        s.kind,
        s.level_db,
        s.delay_ms,
        s.drift_ppm,
        s.singer,
        s.guide,
        s.gap.is_some()
    );
    let _ = write!(
        json,
        "\"true_level_db\":{},\"true_delay_end_ms\":{:.4},\"true_delay_mean_ms\":{:.4},\"presence\":\"{}\",\"first_present_s\":{},\"settled_present_s\":{},\"first_absent_s\":{},\"first_quality_s\":{},",
        opt(Some(true_level)),
        true_delay_end,
        true_delay_mean,
        pres,
        opt(first_present),
        opt(settled_present),
        opt(first_absent),
        opt(first_quality)
    );
    let _ = write!(
        json,
        "\"level_db\":{},\"delay_ms\":{},\"drift_ppm\":{},\"wander_ms\":{},\"stability\":\"{}\",\"confidence\":{:.3},\"quality\":\"{}\",\"relocks\":{},\"filter_resets\":{},\"est_suppression_db\":{},\"true_suppression_db\":{},\"residual_dbfs\":{},\"est_residual_dbfs\":{},",
        opt(rep.level_db.map(f64::from)),
        opt(rep.delay_ms),
        opt(rep.drift_ppm),
        opt(rep.wander_ms),
        stab(rep.stability),
        rep.confidence,
        q,
        rep.relocks,
        ec.filter_resets(),
        opt(rep.suppression_db.map(f64::from)),
        opt(erle),
        opt(residual_dbfs),
        opt(rep.residual_dbfs.map(f64::from))
    );
    let _ = write!(
        json,
        "\"acc_clean\":{:.4},\"acc_raw\":{:.4},\"acc_out\":{:.4},\"acc_out_trusted\":{:.4},\"acc_out_steady\":{:.4},\"acc_out_steady_trusted\":{:.4},\"rest_pitch_raw\":{:.4},\"rest_pitch_out_trusted\":{:.4},",
        clean.acc(),
        raw.acc(),
        out_all.acc(),
        out_trusted.acc(),
        out_steady_all.acc(),
        out_steady_trusted.acc(),
        raw.rest_rate(),
        out_trusted.rest_rate()
    );
    let _ = write!(
        json,
        "\"false_marker_times\":{:?},\"pitch_raw\":{:.4},\"pitch_raw_steady\":{:.4},\"pitch_out\":{:.4},\"pitch_out_trusted\":{:.4},\"pitch_out_steady\":{:.4},\"pitch_out_steady_trusted\":{:.4},\"trust_clean\":{},\"trust_singer\":{},\"trust_music\":{},\"trust_unknown\":{},\"cpu_s_per_s\":{:.5}}}",
        false_times,
        raw.pitch_rate(),
        raw_steady.pitch_rate(),
        out_all.pitch_rate(),
        out_trusted.pitch_rate(),
        out_steady_all.pitch_rate(),
        out_steady_trusted.pitch_rate(),
        trust_counts[Trust::Clean as usize],
        trust_counts[Trust::Singer as usize],
        trust_counts[Trust::Music as usize],
        trust_counts[Trust::Unknown as usize],
        proc_s / secs
    );
    let line = format!(
        "{:<38} det {:>5} lvl {:>6}/{:>6} dly {:>8}/{:>8} {:<9} sup {:>5}/{:>5} q {:<9} acc {:.2}/{:.2}/{:.2} pitch(no singer) raw {:.2} out-trusted {:.3}",
        s.name,
        first_present.map_or("-".into(), |v| format!("{v:.2}")),
        rep.level_db.map_or("-".into(), |v| format!("{v:.1}")),
        format!("{true_level:.1}"),
        rep.delay_ms.map_or("-".into(), |v| format!("{v:.2}")),
        format!("{true_delay_end:.2}"),
        stab(rep.stability),
        rep.suppression_db.map_or("-".into(), |v| format!("{v:.1}")),
        erle.map_or("-".into(), |v| format!("{v:.1}")),
        q,
        clean.acc(),
        raw.acc(),
        out_trusted.acc(),
        raw.pitch_rate(),
        out_trusted.pitch_rate()
    );
    Outcome { json, line }
}

/// CPU per second of audio of the canceller alone, single thread: 60 s of music, a cable
/// return at 0 dB and a singer, 10 ms blocks, at `rate`.
fn cpu(rate: u32) -> String {
    let secs = 60.0;
    let music = testpath::synth_music(rate, secs, -14.0, None, 3);
    let (echo, _) = testpath::apply(&ReturnPath::cable(0.0, 0.008, 100.0), &music, 2, rate);
    let voice = tv::sing(rate, secs, Voice::default(), melody);
    let input: Vec<f32> = echo.iter().zip(&voice).map(|(a, b)| a + b).collect();
    let mut best = f64::MAX;
    for _ in 0..3 {
        let mut ec = EchoCanceller::new(rate);
        let blk = (rate / 100) as usize;
        let mut out = vec![0.0f32; blk];
        let t0 = Instant::now();
        let mut i = 0;
        while i + blk <= input.len() {
            ec.process(&music[2 * i..2 * (i + blk)], &input[i..i + blk], &mut out);
            i += blk;
        }
        best = best.min(t0.elapsed().as_secs_f64());
    }
    format!(
        "{{\"cpu_rate\":{rate},\"seconds\":{secs},\"ms_per_s\":{:.3},\"percent_of_one_core\":{:.3}}}",
        best / secs * 1e3,
        best / secs * 100.0
    )
}

fn scenarios(track: &str, full: bool) -> Vec<Scenario> {
    let mut v = Vec::new();
    let mut add = |kind: &'static str,
                   level_db: f64,
                   delay_ms: f64,
                   drift_ppm: f64,
                   singer: bool,
                   guide: bool,
                   gap: Option<(f64, f64)>| {
        let name = format!(
            "{kind}{}{}_{:+}dB_{}ms{}{}",
            if guide { "+guide" } else { "" },
            if gap.is_some() { "+break" } else { "" },
            level_db,
            delay_ms,
            if drift_ppm != 0.0 && kind == "cable" {
                format!("_{drift_ppm:+}ppm")
            } else {
                String::new()
            },
            if singer { "_singer" } else { "_nosinger" }
        );
        v.push(Scenario {
            name,
            track: track.to_string(),
            kind,
            level_db,
            delay_ms,
            drift_ppm,
            singer,
            guide,
            gap,
        });
    };
    for singer in [false, true] {
        if full {
            for level in [-40.0, -30.0, -20.0, -10.0, 0.0] {
                for delay in [2.0, 10.0, 50.0, 150.0, 300.0] {
                    add("usb", level, delay, 0.0, singer, false, None);
                }
            }
            for level in [-20.0, 0.0] {
                add("usb_eq", level, 12.0, 0.0, singer, false, None);
            }
            for level in [-30.0, -20.0, -10.0, 0.0] {
                for drift in [-100.0, 100.0] {
                    add("cable", level, 7.0, drift, singer, false, None);
                }
            }
            for level in [-20.0, -10.0, 0.0] {
                for delay in [120.0, 200.0, 300.0] {
                    add("bluetooth", level, delay, 0.0, singer, false, None);
                }
            }
            add("usb", 0.0, 10.0, 0.0, singer, true, None);
            add("usb", -10.0, 10.0, 0.0, singer, true, None);
            add("cable", 0.0, 7.0, 100.0, singer, true, None);
            add("cable", -20.0, 7.0, 100.0, singer, true, None);
            add("bluetooth", 0.0, 200.0, 0.0, singer, true, None);
            add("none", 0.0, 0.0, 0.0, singer, false, None);
            add("none", 0.0, 0.0, 0.0, singer, true, None);
            add("usb", -10.0, 10.0, 0.0, singer, false, Some((15.0, 23.0)));
            add("cable", 0.0, 7.0, 100.0, singer, true, Some((15.0, 23.0)));
        } else {
            add("usb", -20.0, 10.0, 0.0, singer, false, None);
            add("cable", 0.0, 7.0, 100.0, singer, false, None);
            add("cable", -10.0, 7.0, -100.0, singer, true, None);
            add("bluetooth", 0.0, 200.0, 0.0, singer, false, None);
            add("none", 0.0, 0.0, 0.0, singer, true, None);
        }
    }
    v
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let get = |k: &str| {
        args.iter()
            .position(|a| a == k)
            .and_then(|i| args.get(i + 1))
            .cloned()
    };
    let has = |k: &str| args.iter().any(|a| a == k);
    let out_dir = get("--out").unwrap_or_else(|| ".".into());
    std::fs::create_dir_all(&out_dir).expect("output folder");
    if has("--cpu") {
        let mut s = String::new();
        for rate in [16_000, 44_100, 48_000, 96_000] {
            let line = cpu(rate);
            println!("{line}");
            s.push_str(&line);
            s.push('\n');
        }
        std::fs::write(format!("{out_dir}/cpu.jsonl"), s).expect("write cpu.jsonl");
        return;
    }
    let tracks_dir = get("--tracks").expect("--tracks <dir>");
    let main_track = get("--main").unwrap_or_else(|| "ccm-22364".into());
    let others: Vec<String> = get("--others")
        .map(|s| s.split(',').map(str::to_string).collect())
        .unwrap_or_default();
    let secs: f64 = get("--secs").and_then(|v| v.parse().ok()).unwrap_or(45.0);
    let start: f64 = get("--start").and_then(|v| v.parse().ok()).unwrap_or(30.0);
    let quick = has("--quick");
    let threads: usize = get("--threads").and_then(|v| v.parse().ok()).unwrap_or(8);
    let mut jobs: Vec<(Scenario, u32, Vec<f32>, Vec<f32>)> = Vec::new();
    let mut tracks = vec![(main_track.clone(), !quick)];
    tracks.extend(others.iter().map(|t| (t.clone(), false)));
    for (t, full) in tracks {
        let dir = format!("{tracks_dir}/{t}");
        let inst = read_wav(&format!("{dir}/instrumental.wav")).expect("instrumental");
        let back = read_wav(&format!("{dir}/backing.wav")).expect("backing");
        let lead = read_wav(&format!("{dir}/lead.wav")).expect("lead");
        let rate = inst.rate;
        let mut music = excerpt(&inst, start, secs);
        for (m, b) in music.iter_mut().zip(excerpt(&back, start, secs)) {
            *m += b;
        }
        let mut guide = excerpt(&lead, start, secs);
        // The backing track at -14 dBFS RMS (mono mix), the guide at its level in the song.
        let g = 10f64.powf(-14.0 / 20.0) / testpath::mono_power(&music, 2).sqrt();
        for v in music.iter_mut().chain(guide.iter_mut()) {
            *v = (f64::from(*v) * g) as f32;
        }
        for s in scenarios(&t, full) {
            jobs.push((s, rate, music.clone(), guide.clone()));
        }
    }
    eprintln!("{} scenarios on {threads} threads", jobs.len());
    let results = std::sync::Mutex::new(Vec::<(usize, Outcome)>::new());
    let next = std::sync::atomic::AtomicUsize::new(0);
    std::thread::scope(|sc| {
        for _ in 0..threads {
            sc.spawn(|| {
                loop {
                    let k = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    let Some((s, rate, music, lead)) = jobs.get(k) else {
                        break;
                    };
                    let o = run(s, *rate, music, lead);
                    eprintln!("{}", o.line);
                    results.lock().expect("results").push((k, o));
                }
            });
        }
    });
    let mut res = results.into_inner().expect("results");
    res.sort_by_key(|(k, _)| *k);
    let mut jsonl = String::new();
    for (_, o) in &res {
        jsonl.push_str(&o.json);
        jsonl.push('\n');
        println!("{}", o.line);
    }
    let name = if quick {
        "results-quick.jsonl"
    } else {
        "results.jsonl"
    };
    std::fs::write(format!("{out_dir}/{name}"), jsonl).expect("write results");
}

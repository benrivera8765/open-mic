// SPDX-License-Identifier: GPL-3.0-or-later
//! Command-line front end of the guide builder, for the benchmark
//! (studio/bench/practice_guide_bench.py) and for looking at one vocal by hand.
//!
//! ```text
//! cargo run --release -p jk-pitch --example guide_wav -- --wav vocal.wav --out guide.json
//!     [--curve other_tracker.csv]   # use this pitch curve (t,f0_hz,conf) instead of pYIN
//!     [--curve-out curve.csv]       # write the curve used (t,f0_hz,conf,level_db)
//!     [--repeat N]                  # run N times, report the fastest (speed benchmark)
//! ```
//!
//! Reads 16/24/32-bit PCM or 32-bit float WAV (also WAVE_FORMAT_EXTENSIBLE), any rate
//! and channel count. Writes the guide as JSON. No dependencies beyond jk-pitch.

use jk_pitch::guide::{self, GuideConfig, NoteKind, OctaveCheck, PitchCurve};
use std::fmt::Write as _;
use std::time::Instant;

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
                fmt = u16le(body + 24); // sub-format GUID starts with the format tag
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
                (1, 32) => d
                    .chunks_exact(4)
                    .map(|s| i32::from_le_bytes([s[0], s[1], s[2], s[3]]) as f32 / 2_147_483_648.0)
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

/// Reads `t,f0_hz,conf` lines (a header line and extra columns are ignored) and puts them
/// on the guide's frame grid (nearest input frame).
fn read_curve(path: &str, hop: f64, frames: usize) -> Result<PitchCurve, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))?;
    let mut rows: Vec<(f64, f32, f32)> = Vec::new();
    for line in text.lines() {
        let f: Vec<&str> = line.split(',').collect();
        if f.len() < 3 {
            continue;
        }
        if let (Ok(t), Ok(f0), Ok(c)) = (
            f[0].trim().parse::<f64>(),
            f[1].trim().parse::<f32>(),
            f[2].trim().parse::<f32>(),
        ) {
            rows.push((
                t,
                if f0.is_finite() && f0 > 0.0 { f0 } else { 0.0 },
                if c.is_finite() { c } else { 0.0 },
            ));
        }
    }
    if rows.is_empty() {
        return Err(format!("{path}: no curve rows"));
    }
    rows.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut f0 = Vec::with_capacity(frames);
    let mut conf = Vec::with_capacity(frames);
    let mut j = 0;
    for k in 0..frames {
        let t = k as f64 * hop;
        while j + 1 < rows.len() && (rows[j + 1].0 - t).abs() <= (rows[j].0 - t).abs() {
            j += 1;
        }
        f0.push(rows[j].1);
        conf.push(rows[j].2.clamp(0.0, 1.0));
    }
    Ok(PitchCurve {
        hop_s: hop,
        f0_hz: f0,
        conf,
        level_db: Vec::new(),
    })
}

fn kind(k: NoteKind) -> &'static str {
    match k {
        NoteKind::Pitched => "pitched",
        NoteKind::Unpitched => "unpitched",
    }
}

fn octave(o: OctaveCheck) -> &'static str {
    match o {
        OctaveCheck::Ok => "ok",
        OctaveCheck::MaybeHigh => "maybe_high",
        OctaveCheck::MaybeLow => "maybe_low",
        OctaveCheck::Unchecked => "unchecked",
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let get = |name: &str| {
        args.iter()
            .position(|a| a == name)
            .and_then(|i| args.get(i + 1))
            .cloned()
    };
    let (Some(wav_path), Some(out_path)) = (get("--wav"), get("--out")) else {
        eprintln!(
            "usage: guide_wav --wav in.wav --out guide.json [--curve c.csv] [--curve-out c.csv] [--repeat N]"
        );
        std::process::exit(2);
    };
    let curve_in = get("--curve");
    let repeat: usize = get("--repeat")
        .and_then(|s| s.parse().ok())
        .unwrap_or(1)
        .max(1);
    let mut cfg = GuideConfig::default();
    // --set name=value (repeatable): benchmark sweeps of the tunables.
    for (i, a) in args.iter().enumerate() {
        if a == "--set"
            && let Some((k, v)) = args.get(i + 1).and_then(|kv| kv.split_once('='))
            && !(v.parse::<f64>().is_ok_and(|x| cfg.set(k, x)))
        {
            eprintln!("--set {k}={v}: unknown tunable or bad value");
            std::process::exit(2);
        }
    }

    let t0 = Instant::now();
    let wav = read_wav(&wav_path).unwrap_or_else(|e| {
        eprintln!("{e}");
        std::process::exit(1)
    });
    let t_decode = t0.elapsed().as_secs_f64();
    let duration = wav.samples.len() as f64 / wav.channels as f64 / wav.rate as f64;

    let mut best: Option<(f64, f64, f64, guide::Guide)> = None;
    for _ in 0..repeat {
        let t1 = Instant::now();
        let a = guide::prepare(&wav.samples, wav.channels, wav.rate);
        let t_prep = t1.elapsed().as_secs_f64();
        let t2 = Instant::now();
        let curve = match &curve_in {
            None => guide::pitch_curve(&a, &cfg),
            Some(p) => {
                let frames =
                    ((a.samples.len() as f64 / a.sample_rate) / cfg.hop_s).floor() as usize + 1;
                let mut c = read_curve(p, cfg.hop_s, frames).unwrap_or_else(|e| {
                    eprintln!("{e}");
                    std::process::exit(1)
                });
                c.level_db = guide::levels(&a, &cfg, frames);
                c
            }
        };
        let t_pitch = t2.elapsed().as_secs_f64();
        let t3 = Instant::now();
        let g = guide::segment(curve, &a, &cfg);
        let t_notes = t3.elapsed().as_secs_f64();
        if best
            .as_ref()
            .is_none_or(|b| t_prep + t_pitch + t_notes < b.0 + b.1 + b.2)
        {
            best = Some((t_prep, t_pitch, t_notes, g));
        }
    }
    let (t_prep, t_pitch, t_notes, g) = best.expect("at least one run");

    let mut s = String::new();
    let _ = write!(
        s,
        "{{\"method\":\"{}\",\"version\":\"{}\",\"tracker\":\"{}\",\"sample_rate\":{},\"channels\":{},\"duration_s\":{:.3},",
        g.method,
        g.version,
        if curve_in.is_some() {
            "external_curve"
        } else {
            "pyin"
        },
        wav.rate,
        wav.channels,
        duration
    );
    let _ = write!(
        s,
        "\"tuning_cents\":{:.2},\"tuning_strength\":{:.3},\"octave_flags\":{},\"timing_s\":{{\"decode\":{:.4},\"prepare\":{:.4},\"pitch\":{:.4},\"notes\":{:.4},\"build_total\":{:.4},\"repeat\":{}}},\"notes\":[",
        g.tuning_cents,
        g.tuning_strength,
        g.octave_flags,
        t_decode,
        t_prep,
        t_pitch,
        t_notes,
        t_prep + t_pitch + t_notes,
        repeat
    );
    for (i, n) in g.notes.iter().enumerate() {
        let _ = write!(
            s,
            "{}{{\"start\":{:.3},\"end\":{:.3},\"midi\":{:.3},\"conf\":{:.3},\"kind\":\"{}\",\"octave\":\"{}\",\"glide_in\":{:.3}}}",
            if i > 0 { "," } else { "" },
            n.start_s,
            n.end_s,
            n.midi,
            n.confidence,
            kind(n.kind),
            octave(n.octave),
            n.glide_in_s
        );
    }
    s.push_str("]}\n");
    std::fs::write(&out_path, s).unwrap_or_else(|e| {
        eprintln!("{out_path}: {e}");
        std::process::exit(1)
    });
    if let Some(p) = get("--curve-out") {
        let mut c = String::from("t,f0_hz,conf,level_db\n");
        for k in 0..g.curve.len() {
            let _ = writeln!(
                c,
                "{:.3},{:.3},{:.3},{:.1}",
                k as f64 * g.curve.hop_s,
                g.curve.f0_hz[k],
                g.curve.conf[k],
                g.curve.level_db[k]
            );
        }
        std::fs::write(&p, c).unwrap_or_else(|e| {
            eprintln!("{p}: {e}");
            std::process::exit(1)
        });
    }
    eprintln!(
        "{}: {:.1} s audio, {} notes ({} unpitched), tuning {:+.1} c, build {:.3} s",
        wav_path,
        duration,
        g.notes.len(),
        g.notes
            .iter()
            .filter(|n| n.kind == NoteKind::Unpitched)
            .count(),
        g.tuning_cents,
        t_prep + t_pitch + t_notes
    );
}

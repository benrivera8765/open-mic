// SPDX-License-Identifier: GPL-3.0-or-later
//! Key-change accuracy (WORK_ORDER section 9): synthetic tones shifted by every step
//! -12..+12. Measures pitch error in cents, output length, onset positions and peak level.

mod common;
use common::*;
use jk_dsp::analysis::{cents, estimate_frequency, onset, peak_abs};
use jk_dsp::signals::{harmonic, sine};
use jk_dsp::{MixParams, Stem};

const RATE: u32 = 44_100;

#[derive(Clone, Copy)]
enum Tone {
    Sine(f64),
    Harmonic(f64),
}

impl Tone {
    fn f0(self) -> f64 {
        match self {
            Tone::Sine(f) | Tone::Harmonic(f) => f,
        }
    }
    fn name(self) -> String {
        match self {
            Tone::Sine(f) => format!("sine {f} Hz"),
            Tone::Harmonic(f) => format!("harmonic {f} Hz (6 partials)"),
        }
    }
    fn make(self, frames: usize, amp: f64) -> Vec<f32> {
        match self {
            Tone::Sine(f) => sine(f, RATE, frames, amp, 0.0),
            Tone::Harmonic(f) => harmonic(f, &[1.0, 0.7, 0.5, 0.35, 0.25, 0.15], RATE, frames, amp),
        }
    }
}

/// Three tone bursts (2 ms attack, 5 ms release) starting at these frames.
const BURSTS: [usize; 3] = [22_050, 55_125, 88_200];
const BURST_LEN: usize = 17_640; // 0.4 s

fn bursts(tone: Tone, frames: usize) -> Vec<f32> {
    let t = tone.make(frames, 0.5);
    let mut out = vec![0.0f32; frames];
    let att = (RATE as usize * 2) / 1000;
    let rel = (RATE as usize * 5) / 1000;
    for &b in &BURSTS {
        for i in 0..BURST_LEN {
            let env = if i < att {
                i as f32 / att as f32
            } else if i >= BURST_LEN - rel {
                (BURST_LEN - i) as f32 / rel as f32
            } else {
                1.0
            };
            out[b + i] = t[b + i] * env;
        }
    }
    out
}

struct Row {
    semis: i32,
    cents_err: f64,
    len_ok: bool,
    peak: f32,
    onset_err: Vec<i64>,
}

fn measure(tone: Tone, semis: i32) -> Row {
    let frames = 3 * RATE as usize;
    // Steady tone in the "other" stem (it goes through the pitch bus).
    let mut stems = silent_stems(frames);
    let x = tone.make(frames, 0.5);
    stems[Stem::Other as usize] = [x.clone(), x];
    let params = MixParams { semitones: semis, ..MixParams::default() };
    let out = render_native(&stems, RATE, params, &[]);
    let len_ok = out.channels[0].len() == frames && out.channels[1].len() == frames;
    assert_eq!(out.stats.shift_shortfall_frames, 0);
    let want = tone.f0() * (semis as f64 / 12.0).exp2();
    let seg = &out.channels[0][RATE as usize..2 * RATE as usize];
    let got = estimate_frequency(seg, RATE, want, 60.0);
    let peak = peak_abs(&out.channels[0]).max(peak_abs(&out.channels[1]));

    // Onsets.
    let b = bursts(tone, frames);
    let mut stems = silent_stems(frames);
    stems[Stem::Other as usize] = [b.clone(), b.clone()];
    let out_b = render_native(&stems, RATE, params, &[]);
    let w = RATE as usize / 5;
    let onset_err = BURSTS
        .iter()
        .map(|&p| {
            let r = p - w..p + w;
            let i_in = onset(&b, RATE, r.clone(), 0.5).unwrap() as i64;
            let i_out = onset(&out_b.channels[0], RATE, r, 0.5).unwrap() as i64;
            i_out - i_in
        })
        .collect();
    Row { semis, cents_err: cents(got, want), len_ok, peak, onset_err }
}

#[test]
fn key_change_accuracy_all_steps() {
    let tones = [Tone::Sine(110.0), Tone::Sine(440.0), Tone::Sine(1760.0), Tone::Harmonic(220.0)];
    let mut rep = Report::new("key_change_accuracy");
    rep.line("Key change accuracy, 44.1 kHz stereo, input -6.02 dBFS peak, Rubber Band R3 (HighQuality), every step -12..+12");
    rep.line("pitch: frequency of the strongest component near the target, 1.0-2.0 s of a 3 s tone (Blackman-Harris DTFT peak)");
    rep.line("onset: 50 % point of the 1 ms RMS envelope, output minus input, 3 bursts with 2 ms attack");
    let mut worst_cents = 0.0f64;
    let mut worst_onset = 0i64;
    let mut worst_peak = 0.0f32;
    for tone in tones {
        let rows: Vec<Row> = std::thread::scope(|s| {
            let hs: Vec<_> = (-12..=12).map(|semis| s.spawn(move || measure(tone, semis))).collect();
            hs.into_iter().map(|h| h.join().unwrap()).collect()
        });
        rep.line(format!("-- {}", tone.name()));
        rep.line("semis | pitch error (cents) | length exact | output peak (dBFS) | onset error per burst (samples)");
        let mut tmax = 0.0f64;
        for r in &rows {
            rep.line(format!(
                "{:+3} | {:+8.4} | {} | {:7.2} | {:?}",
                r.semis,
                r.cents_err,
                if r.len_ok { "yes" } else { "NO" },
                20.0 * (r.peak as f64).log10(),
                r.onset_err
            ));
            tmax = tmax.max(r.cents_err.abs());
            worst_onset = worst_onset.max(r.onset_err.iter().map(|e| e.abs()).max().unwrap());
            worst_peak = worst_peak.max(r.peak);
            assert!(r.len_ok, "output length changed");
        }
        rep.line(format!("max |pitch error| for {}: {:.4} cents", tone.name(), tmax));
        worst_cents = worst_cents.max(tmax);
    }
    rep.line(format!(
        "SUMMARY: max |pitch error| {worst_cents:.4} cents; max |onset error| {worst_onset} samples ({:.2} ms); max output peak {:.2} dBFS (input -6.02 dBFS)",
        worst_onset as f64 * 1000.0 / RATE as f64,
        20.0 * (worst_peak as f64).log10()
    ));
    assert!(worst_cents < 5.0, "pitch error above 5 cents");
    assert!(worst_peak < 1.0, "output clipped");
    assert!(worst_onset as f64 <= 0.010 * RATE as f64, "onset moved by more than 10 ms");
}

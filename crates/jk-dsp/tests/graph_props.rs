// SPDX-License-Identifier: GPL-3.0-or-later
//! Graph properties: drum alignment (WORK_ORDER section 7 trap 6), transparency at 0
//! semitones, click-free key change during playback, no shifter shortfall, block-size
//! independence.

mod common;
use common::*;
use jk_dsp::analysis::{db, impulse_alignment, max_first_diff, max_second_diff, rms_envelope};
use jk_dsp::signals::{sine, synth_song};
use jk_dsp::{MixParams, ParamEvent, Stem};

#[test]
fn drum_alignment_impulse() {
    let mut rep = Report::new("drum_alignment");
    rep.line("Drum alignment: 64 isolated impulses per shift (stride-29 positions: a different set from the stride-17/23 calibration sets),");
    rep.line("each sent once through the pitch path (backing stem) and once through the drum path; offset = band peak - drum peak (samples)");
    rep.line("rate | semis | drum path offset (all impulses) | band-drum offset: mean, min, max");
    for &rate in &[44_100u32, 48_000, 96_000] {
        let shifts: Vec<i32> = if rate == 44_100 {
            (-12..=12).filter(|s| *s != 0).collect()
        } else {
            vec![-12, -7, -5, -2, 2, 5, 7, 12]
        };
        let rows: Vec<_> = std::thread::scope(|s| {
            let hs: Vec<_> = shifts
                .iter()
                .map(|&semis| s.spawn(move || (semis, impulse_alignment(rate, semis, 64, 29).unwrap())))
                .collect();
            hs.into_iter().map(|h| h.join().unwrap()).collect()
        });
        let mut worst_mean = 0.0f64;
        for (semis, a) in &rows {
            let drum_ok = a.drum_offsets.iter().all(|&d| d == 0);
            rep.line(format!(
                "{rate} | {semis:+3} | {} | {:+7.2}, {:+5}, {:+5}",
                if drum_ok { "0 (exact)" } else { "NOT 0" },
                a.mean(),
                a.min(),
                a.max()
            ));
            assert!(drum_ok, "drum path moved an impulse");
            worst_mean = worst_mean.max(a.mean().abs());
        }
        rep.line(format!("{rate}: worst |mean band-drum offset| = {worst_mean:.2} samples ({:.3} ms)", worst_mean * 1000.0 / rate as f64));
        // The drum path is exact by construction. The band's transients carry Rubber Band's
        // hop-grid jitter (min/max above); its mean is calibrated to within 0.5 ms.
        assert!(worst_mean <= 0.0005 * rate as f64, "band/drum mean offset above 0.5 ms");
    }
}

#[test]
fn transparent_at_zero_semitones() {
    let rate = 44_100;
    let stems = synth_song(rate, 6.0, 7);
    let mut rep = Report::new("transparency");
    for (label, params) in [
        ("unity gains, guide 1.0", MixParams { guide: 1.0, ..MixParams::default() }),
        (
            "guide 0.35, backing 0.8, other 0.9, drums 1.1",
            MixParams { guide: 0.35, backing: 0.8, other: 0.9, drums: 1.1, ..MixParams::default() },
        ),
        ("shift_drums on (no effect at 0)", MixParams { guide: 1.0, shift_drums: true, ..MixParams::default() }),
    ] {
        let out = render_native(&stems, rate, params, &[]);
        let g = params.stem_gains();
        let mut max_same = 0.0f32;
        let mut max_naive = 0.0f32;
        for c in 0..2 {
            for i in 0..stems[0][0].len() {
                let lead = stems[Stem::Lead as usize][c][i];
                let back = stems[Stem::Backing as usize][c][i];
                let drum = stems[Stem::Drums as usize][c][i];
                let other = stems[Stem::Other as usize][c][i];
                // The plain mix, in the graph's summation order.
                let plain = (lead * g[0] + back * g[1] + other * g[3]) + drum * g[2];
                // The same mix summed in a different order (shows the 1-ulp scale).
                let naive = lead * g[0] + back * g[1] + drum * g[2] + other * g[3];
                max_same = max_same.max((out.channels[c][i] - plain).abs());
                max_naive = max_naive.max((out.channels[c][i] - naive).abs());
            }
        }
        rep.line(format!(
            "{label}: max |output - plain mix| = {max_same:e} (same summation order); vs another summation order = {max_naive:e}"
        ));
        assert_eq!(max_same, 0.0, "0 semitones must be bit-transparent");
    }
}

#[test]
fn key_change_during_playback_is_click_free() {
    let rate = 44_100u32;
    let frames = 4 * rate as usize;
    let amp = 0.5;
    let x = sine(440.0, rate, frames, amp, 0.0);
    let mut stems = silent_stems(frames);
    stems[Stem::Other as usize] = [x.clone(), x];
    let p = |s| MixParams { semitones: s, ..MixParams::default() };
    let changes = [(rate as u64, 2), (2 * rate as u64, -3), (3 * rate as u64, 0)];
    let events: Vec<ParamEvent> = changes.iter().map(|&(f, s)| ParamEvent { frame: f, params: p(s) }).collect();
    let out = render_native(&stems, rate, p(0), &events);
    assert_eq!(out.stats.shift_shortfall_frames, 0);
    assert_eq!(out.stats.transitions, 3);
    let y = &out.channels[0];
    let xf = (0.040 * rate as f64) as usize;
    let guard = rate as usize / 100; // 10 ms either side of the crossfade
    let mut rep = Report::new("key_change_click");
    rep.line("Key change 0 -> +2 -> -3 -> 0 at 1 s, 2 s, 3 s on a 440 Hz sine (-6 dBFS), 44.1 kHz, 40 ms equal-power crossfade");
    rep.line("d1 = max |x[n]-x[n-1]|, d2 = max |x[n]-2x[n-1]+x[n-2]|; 'steady' = the 0.5 s before the change, at the higher of the two keys");
    // Steady reference: the largest d1/d2 of either neighbouring steady segment.
    let steady = |a: usize, b: usize| (max_first_diff(y, a..b), max_second_diff(y, a..b));
    let seg = [
        (rate as usize / 4, rate as usize - guard),
        (rate as usize + xf + guard, 2 * rate as usize - guard),
        (2 * rate as usize + xf + guard, 3 * rate as usize - guard),
        (3 * rate as usize + xf + guard, 4 * rate as usize - rate as usize / 4),
    ];
    let env = rms_envelope(y, rate as usize / 100);
    let steady_rms = rms_envelope(y, rate as usize / 100)[rate as usize / 2];
    let mut worst_ratio1 = 0.0f32;
    let mut worst_ratio2 = 0.0f32;
    for (k, &(f, s)) in changes.iter().enumerate() {
        let f = f as usize;
        let (a1, a2) = steady(seg[k].0, seg[k].1);
        let (b1, b2) = steady(seg[k + 1].0, seg[k + 1].1);
        let (r1, r2) = (a1.max(b1), a2.max(b2));
        let t1 = max_first_diff(y, f - guard..f + xf + guard);
        let t2 = max_second_diff(y, f - guard..f + xf + guard);
        let emin = env[f - guard..f + xf + guard].iter().cloned().fold(f32::MAX, f32::min);
        let emax = env[f - guard..f + xf + guard].iter().cloned().fold(0.0f32, f32::max);
        rep.line(format!(
            "change to {s:+} at {:.1} s: transition d1 {t1:.5} vs steady {r1:.5} (ratio {:.3}); d2 {t2:.6} vs steady {r2:.6} (ratio {:.3}); 10 ms RMS during transition {:+.2} .. {:+.2} dB re steady",
            f as f64 / rate as f64,
            t1 / r1,
            t2 / r2,
            db(emin as f64 / steady_rms as f64),
            db(emax as f64 / steady_rms as f64),
        ));
        worst_ratio1 = worst_ratio1.max(t1 / r1);
        worst_ratio2 = worst_ratio2.max(t2 / r2);
    }
    rep.line(format!("SUMMARY: worst transition/steady ratio: d1 {worst_ratio1:.3}, d2 {worst_ratio2:.3}"));
    // A click is a step: it shows up as a d2 many times the sine's own curvature.
    assert!(worst_ratio2 < 2.0, "click detected at a key change");
    assert!(worst_ratio1 < 1.6, "discontinuity at a key change");
}

#[test]
fn no_shortfall_and_block_size_independent() {
    let mut rep = Report::new("block_independence");
    for &rate in &[44_100u32, 48_000, 96_000] {
        let stems = synth_song(rate, 4.0, 3);
        let p = |s, d| MixParams { semitones: s, shift_drums: d, guide: 0.5, ..MixParams::default() };
        let events = [
            ParamEvent { frame: rate as u64 / 2, params: p(12, false) },
            ParamEvent { frame: rate as u64, params: p(-12, false) },
            ParamEvent { frame: 3 * rate as u64 / 2, params: p(-12, true) },
            ParamEvent { frame: 2 * rate as u64, params: p(5, true) },
            ParamEvent { frame: 5 * rate as u64 / 2, params: MixParams { backing_on: false, ..p(5, true) } },
            ParamEvent { frame: 3 * rate as u64, params: p(0, false) },
        ];
        let reference = render_block(&stems, rate, p(-7, false), &events, 512);
        assert_eq!(reference.stats.shift_shortfall_frames, 0);
        for block in [64usize, 333, 1024, 4096] {
            let o = render_block(&stems, rate, p(-7, false), &events, block);
            assert_eq!(o.stats.shift_shortfall_frames, 0, "shortfall at block {block}");
            let mut maxd = 0.0f32;
            for c in 0..2 {
                for (a, b) in o.channels[c].iter().zip(&reference.channels[c]) {
                    maxd = maxd.max((a - b).abs());
                }
            }
            rep.line(format!("{rate} Hz, block {block} vs 512: max |diff| = {maxd:e}, shortfall frames = 0"));
            assert_eq!(maxd, 0.0, "output depends on block size");
        }
    }
}

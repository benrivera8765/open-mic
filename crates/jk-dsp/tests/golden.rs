// SPDX-License-Identifier: GPL-3.0-or-later
//! Determinism and the cross-platform golden files (WORK_ORDER section 9, "same audio on
//! both systems").
//!
//! * Rendering the same input twice gives byte-identical output.
//! * The output is compared with `tests/golden/*.f32` (little-endian f32, interleaved
//!   stereo), produced on the reference platform (Windows x86-64, GNU toolchain):
//!   - on the reference platform the SHA-256 must match exactly (set `JK_GOLDEN_STRICT=0`
//!     to downgrade to the tolerance check, e.g. for the perturbation experiment);
//!   - everywhere (macOS ARM64 in CI) the samples must match within [`TOL_MAX_ABS`] and
//!     [`TOL_RMS_DB`].
//! * `JK_UPDATE_GOLDEN=1` rewrites the files (only on the reference platform, and only when
//!   a change in output is understood and intended).

mod common;
use common::*;
use jk_dsp::signals::synth_song;
use jk_dsp::{render, MixParams, ParamEvent, RenderOptions};
use std::path::PathBuf;

/// Largest allowed |difference| of any sample between platforms (-100 dBFS; the same
/// default as scripts/ci-compare-renders.sh).
/// Basis (docs/benchmarks/dsp.md, "cross-platform tolerance"): rebuilding Rubber Band with
/// x87 80-bit maths instead of SSE2 (`JK_RUBBERBAND_PERTURB=1`) moved the output by at most
/// 1.5e-7 (RMS -139 dB). Expected Windows/macOS differences (last-bit libm results, NEON vs
/// AVX summation order in rubato) are of that order; the limit leaves about 70x margin
/// while still catching a real divergence such as a different FFT or algorithm.
const TOL_MAX_ABS: f32 = 1.0e-5;
/// Largest allowed RMS of the difference, relative to the RMS of the golden signal.
const TOL_RMS_DB: f64 = -110.0;

struct Case {
    name: &'static str,
    seconds: f64,
    out_rate: u32,
    initial: MixParams,
    events: Vec<ParamEvent>,
}

fn cases() -> Vec<Case> {
    let rate = 44_100u64;
    let p = |s: i32| MixParams { guide: 0.25, semitones: s, ..MixParams::default() };
    vec![
        Case {
            // Native rate, three key changes (crossfades, fresh R3 instances, bypass),
            // a drum-routing change and a gain ramp.
            name: "render_a",
            seconds: 1.0,
            out_rate: 44_100,
            initial: p(0),
            events: vec![
                ParamEvent { frame: rate / 5, params: p(3) },
                ParamEvent { frame: rate / 2, params: MixParams { shift_drums: true, ..p(-2) } },
                ParamEvent { frame: 3 * rate / 4, params: MixParams { backing: 0.5, ..p(0) } },
            ],
        },
        Case {
            // Through the output resampler (44.1 -> 48 kHz) at +2.
            name: "render_b",
            seconds: 0.5,
            out_rate: 48_000,
            initial: p(2),
            events: vec![],
        },
    ]
}

fn golden_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests").join("golden")
}

fn do_render(c: &Case) -> [Vec<f32>; 2] {
    let stems = synth_song(44_100, c.seconds, 11);
    let mut o = RenderOptions::native(44_100);
    o.out_rate = c.out_rate;
    let out = render(stems_ref(&stems, 44_100), c.initial, &c.events, &o).unwrap();
    assert_eq!(out.stats.shift_shortfall_frames, 0);
    out.channels
}

fn reference_platform() -> bool {
    cfg!(all(target_os = "windows", target_env = "gnu", target_arch = "x86_64"))
}

#[test]
fn deterministic_and_matches_golden() {
    let mut rep = Report::new("golden");
    let update = std::env::var("JK_UPDATE_GOLDEN").map(|v| v == "1").unwrap_or(false);
    let strict = std::env::var("JK_GOLDEN_STRICT").map(|v| v != "0").unwrap_or(true);
    for c in cases() {
        let a = do_render(&c);
        let b = do_render(&c);
        let bytes = interleaved_le_bytes(&a);
        assert_eq!(bytes, interleaved_le_bytes(&b), "{}: two renders differ", c.name);
        let hash = sha256_hex(&bytes);
        rep.line(format!(
            "{}: {} frames at {} Hz, rendered twice: identical; sha256 {hash}",
            c.name,
            a[0].len(),
            c.out_rate
        ));
        let dir = golden_dir();
        let f32_path = dir.join(format!("{}.f32", c.name));
        let hash_path = dir.join(format!("{}.sha256", c.name));
        if update {
            assert!(reference_platform(), "golden files are generated on the reference platform only");
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(&f32_path, &bytes).unwrap();
            std::fs::write(&hash_path, format!("{hash}  {}.f32\n", c.name)).unwrap();
            rep.line(format!("{}: golden files written", c.name));
            continue;
        }
        let gold = std::fs::read(&f32_path).unwrap_or_else(|_| panic!("missing {f32_path:?} (run with JK_UPDATE_GOLDEN=1 on the reference platform)"));
        let gold_hash = std::fs::read_to_string(&hash_path).unwrap();
        let gold_hash = gold_hash.split_whitespace().next().unwrap().to_string();
        assert_eq!(gold.len(), bytes.len(), "{}: length differs from golden", c.name);
        let g: Vec<f32> = gold.chunks_exact(4).map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]])).collect();
        let o: Vec<f32> = bytes.chunks_exact(4).map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]])).collect();
        let mut max_abs = 0.0f32;
        let (mut e_diff, mut e_sig) = (0.0f64, 0.0f64);
        let mut differing = 0usize;
        for (x, y) in o.iter().zip(&g) {
            let d = (x - y).abs();
            if d > 0.0 {
                differing += 1;
            }
            max_abs = max_abs.max(d);
            e_diff += (d as f64).powi(2);
            e_sig += (*y as f64).powi(2);
        }
        let rms_db = if e_diff == 0.0 { f64::NEG_INFINITY } else { 10.0 * (e_diff / e_sig).log10() };
        rep.line(format!(
            "{}: vs golden: hash {}, samples differing {differing}/{}, max |diff| {max_abs:e}, RMS diff {rms_db:.1} dB re signal (tolerance {TOL_MAX_ABS:e}, {TOL_RMS_DB} dB)",
            c.name,
            if hash == gold_hash { "identical" } else { "DIFFERENT" },
            o.len()
        ));
        assert!(max_abs <= TOL_MAX_ABS, "{}: max |diff| {max_abs} above tolerance", c.name);
        assert!(rms_db <= TOL_RMS_DB, "{}: RMS diff {rms_db} dB above tolerance", c.name);
        if reference_platform() && strict {
            assert_eq!(hash, gold_hash, "{}: not bit-identical on the reference platform", c.name);
        }
    }
}

// SPDX-License-Identifier: GPL-3.0-or-later
//! The key heard, as the graph reports it (Work Order 2, 4.6 and trap 1): every key
//! change is reported by `PitchGraph::scheduled_switch` before any of its audio leaves the
//! graph, the reported frame is where the pitch of the output really changes (measured on
//! a tone), `audible_semitones` switches at that frame, and a cached key that goes missing
//! during its crossfade is reported again with the bypass key.

mod common;
use common::*;
use jk_dsp::analysis::tone_switch_index;
use jk_dsp::signals::sine;
use jk_dsp::{GraphConfig, KeySwitch, MixParams, PitchGraph, Stem, StemBlock, CHANNELS};

const BLOCK: usize = 512;

/// Feed `stems` through `graph` block by block; before each block `hook(graph, input
/// frame)` may change parameters. Returns the output (latency NOT removed: output index
/// `n` carries stream frame `n - latency`) and every switch the graph reported, with the
/// number of output frames already produced when it was first seen.
fn drive(stems: &Stems, graph: &mut PitchGraph, hook: &mut dyn FnMut(&mut PitchGraph, u64)) -> (Vec<f32>, Vec<(KeySwitch, u64)>) {
    let frames = stems[0][0].len() / BLOCK * BLOCK;
    let mut out = Vec::with_capacity(frames);
    let (mut l, mut r) = (vec![0.0f32; BLOCK], vec![0.0f32; BLOCK]);
    let mut seen: Vec<(KeySwitch, u64)> = Vec::new();
    let mut last: Option<KeySwitch> = None;
    let mut pos = 0usize;
    while pos < frames {
        hook(graph, pos as u64);
        let st: [[&[f32]; CHANNELS]; 4] = std::array::from_fn(|s| std::array::from_fn(|c| &stems[s][c][pos..pos + BLOCK]));
        graph.process(&StemBlock { stems: st }, BLOCK, [&mut l[..], &mut r[..]]);
        out.extend_from_slice(&l);
        pos += BLOCK;
        let sw = graph.scheduled_switch();
        if sw != last {
            if let Some(s) = sw {
                seen.push((s, out.len() as u64));
            }
            last = sw;
        }
    }
    (out, seen)
}

#[test]
fn reported_switches_are_where_the_pitch_changes() {
    let mut rep = Report::new("key_switch");
    for &rate in &[44_100u32, 48_000] {
        let f0 = 660.0;
        let frames = 13 * rate as usize;
        let x = sine(f0, rate, frames, 0.4, 0.3);
        let mut stems = silent_stems(frames);
        stems[Stem::Other as usize] = [x.clone(), x];
        let mut cfg = GraphConfig::new(rate);
        cfg.max_block = BLOCK;
        let mut graph = PitchGraph::new(cfg, MixParams::default()).unwrap();
        let lat = graph.latency() as u64;
        // (input frame of the request, key): steps of 2 to 24 semitones, both ends of the range.
        let plan: Vec<(u64, i32)> = [(1.0, 5), (2.5, -7), (4.0, 12), (5.5, -12), (7.0, 0), (8.5, 2), (10.0, -3), (11.5, 0)]
            .iter()
            .map(|&(s, k)| ((s * rate as f64) as u64, k))
            .collect();
        let mut audible_ok = true;
        let mut next = 0usize;
        let (out, seen) = drive(&stems, &mut graph, &mut |g, pos| {
            if next < plan.len() && pos >= plan[next].0 {
                g.set_params(&MixParams { semitones: plan[next].1, ..MixParams::default() });
                next += 1;
            }
            // `audible_semitones` agrees with the switches reported so far: the key of the
            // latest switch at or before the next output frame (stream frame pos - L).
            if let Some(sw) = g.scheduled_switch() {
                let u = pos.saturating_sub(lat);
                if u >= sw.at && g.audible_semitones() != sw.semitones {
                    audible_ok = false;
                }
            }
        });
        assert!(audible_ok, "audible_semitones disagrees with the reported switch");
        assert_eq!(seen.len(), plan.len(), "one reported switch per key change: {seen:?}");
        let mut prev = 0;
        let mut worst = 0.0f64;
        for (k, ((sw, out_when_seen), &(req, key))) in seen.iter().zip(&plan).enumerate() {
            assert_eq!(sw.semitones, key);
            // Reported before any of its audio was output; heard about one graph latency
            // after the request (the request lands at the next block boundary).
            let heard_at = sw.at + lat; // output index carrying stream frame `at`
            assert!(heard_at > *out_when_seen, "switch {k} reported only after it was output");
            let delay_ms = (sw.at as f64 - req as f64) * 1e3 / rate as f64;
            let (fa, fb) = (f0 * (prev as f64 / 12.0).exp2(), f0 * (key as f64 / 12.0).exp2());
            let measured = tone_switch_index(&out, rate, fa, fb, heard_at as usize, rate as usize / 10, rate as usize / 2000)
                .expect("a measurable pitch change");
            let err_ms = (measured - heard_at as f64) * 1e3 / rate as f64;
            worst = worst.max(err_ms.abs());
            rep.line(format!(
                "{rate} Hz: {prev:+} -> {key:+}: reported at stream frame {} ({:.1} ms after the request, {:.1} ms before it was output); pitch change measured {err_ms:+.2} ms from it",
                sw.at,
                delay_ms,
                (heard_at - out_when_seen) as f64 * 1e3 / rate as f64,
            ));
            prev = key;
        }
        rep.line(format!("SUMMARY {rate} Hz: {} key changes, worst |measured - reported| {worst:.2} ms", plan.len()));
        assert!(worst < 5.0, "the reported switch is {worst:.2} ms away from the measured pitch change");
    }
}

#[test]
fn a_cached_key_that_goes_missing_is_reported_again_as_the_bypass() {
    let rate = 44_100u32;
    let frames = 3 * rate as usize;
    let x = sine(440.0, rate, frames, 0.4, 0.0);
    let mut stems = silent_stems(frames);
    stems[Stem::Other as usize] = [x.clone(), x];
    let mut cfg = GraphConfig::new(rate);
    cfg.max_block = BLOCK;
    let mut graph = PitchGraph::new(cfg, MixParams::default()).unwrap();
    graph.set_auto_route(false);
    let lat = graph.latency() as u64;
    let start = rate as u64; // request the cached key here
    let mut events = Vec::new();
    let mut pos = 0usize;
    let zeros = vec![0.0f32; BLOCK];
    let (mut l, mut r) = (vec![0.0f32; BLOCK], vec![0.0f32; BLOCK]);
    let mut missed = false;
    while pos + BLOCK <= frames {
        if pos as u64 == start / BLOCK as u64 * BLOCK as u64 {
            graph.request_cached(0, 4, false);
        }
        // The cached data stops two blocks into the crossfade's pending phase.
        let have = graph.cache_slot_in_use(0) && !missed;
        if graph.is_crossfading() && graph.cache_slot_in_use(0) && events.len() == 1 && !missed {
            graph.cache_miss(0);
            missed = true;
            if let Some(s) = graph.scheduled_switch() {
                events.push(s);
            }
        }
        let st: [[&[f32]; CHANNELS]; 4] = std::array::from_fn(|s| std::array::from_fn(|c| &stems[s][c][pos..pos + BLOCK]));
        let cached = if have { Some([&zeros[..], &zeros[..]]) } else { None };
        graph.process_cached(&StemBlock { stems: st }, None, [cached, None], BLOCK, [&mut l[..], &mut r[..]]);
        if let Some(s) = graph.scheduled_switch()
            && events.last() != Some(&s)
        {
            events.push(s);
        }
        pos += BLOCK;
    }
    assert_eq!(events.len(), 2, "{events:?}");
    assert_eq!(events[0].semitones, 4);
    assert_eq!(events[1], jk_dsp::KeySwitch { at: events[0].at, semitones: 0 });
    assert!(events[0].at + lat > start, "the switch is heard after the request");
    assert_eq!(graph.audible_semitones(), 0);
}

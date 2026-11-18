// SPDX-License-Identifier: GPL-3.0-or-later
//! The cached ("prerendered") route of the graph (WORK_ORDER 4.5, render ahead and cache):
//! pitch-path material rendered by a `pitched_only` graph and played back through the
//! cached route equals the real-time route bit for bit; switching to and from it uses the
//! same equal-power crossfade; a missing block fades to the bypass before the gap.

mod common;
use common::*;
use jk_dsp::signals::synth_song;
use jk_dsp::{EqualPowerTable, GraphConfig, MixParams, PitchGraph, RouteInfo, StemBlock, CHANNELS};

const RATE: u32 = 44_100;
const BLOCK: usize = 512;

/// What to do before the block that starts at song frame `pos`.
type Hook<'a> = &'a mut dyn FnMut(&mut PitchGraph, u64);

/// Run a graph over the whole song from frame 0 and return `frames` frames of output
/// (latency removed). `cache`: pitch-path frames for slot 0 (`None` past `cache_end`).
fn run(
    stems: &Stems,
    graph: &mut PitchGraph,
    cache: Option<(&[Vec<f32>; 2], usize)>,
    hook: Hook,
) -> [Vec<f32>; 2] {
    let frames = stems[0][0].len();
    let lat = graph.latency();
    // Zeros after the end of the song, as the offline renderer feeds them.
    let mut padded = stems.clone();
    for s in padded.iter_mut() {
        for c in s.iter_mut() {
            c.resize(frames + lat + 2 * BLOCK, 0.0);
        }
    }
    // Cached frames up to `end`, zeros after (past the end of the song too).
    let cache = cache.map(|(c, end)| {
        let pad = |v: &Vec<f32>| {
            let mut v = v[..end.min(v.len())].to_vec();
            v.resize(frames + lat + 2 * BLOCK, 0.0);
            v
        };
        ([pad(&c[0]), pad(&c[1])], end)
    });
    let mut out = [Vec::with_capacity(frames), Vec::with_capacity(frames)];
    let (mut l, mut r) = (vec![0.0f32; BLOCK], vec![0.0f32; BLOCK]);
    let mut pos = 0usize;
    while out[0].len() < frames {
        hook(graph, pos as u64);
        let n = BLOCK;
        let st: [[&[f32]; CHANNELS]; 4] = std::array::from_fn(|s| std::array::from_fn(|c| &padded[s][c][pos..pos + n]));
        let cached = match &cache {
            Some((c, end)) if pos < *end => Some([&c[0][pos..pos + n], &c[1][pos..pos + n]]),
            _ => None,
        };
        graph.process_cached(&StemBlock { stems: st }, None, [cached, None], n, [&mut l[..], &mut r[..]]);
        for i in 0..n {
            if pos + i >= lat && out[0].len() < frames {
                out[0].push(l[i]);
                out[1].push(r[i]);
            }
        }
        pos += n;
    }
    out
}

fn graph(params: MixParams, pitched_only: bool) -> PitchGraph {
    let mut cfg = GraphConfig::new(RATE);
    cfg.max_block = BLOCK;
    cfg.pitched_only = pitched_only;
    PitchGraph::new(cfg, params).unwrap()
}

/// The key cache's content for `params`: the pitch path alone, from frame 0.
fn pitched(stems: &Stems, params: MixParams) -> [Vec<f32>; 2] {
    let frames = stems[0][0].len();
    let mut g = graph(params, true);
    run(stems, &mut g, None, &mut |_, _| {}).map(|mut v| {
        v.truncate(frames);
        v
    })
}

fn first_diff(a: &[Vec<f32>; 2], b: &[Vec<f32>; 2], from: usize, to: usize) -> Option<usize> {
    (from..to).find(|&i| a[0][i].to_bits() != b[0][i].to_bits() || a[1][i].to_bits() != b[1][i].to_bits())
}

#[test]
fn cached_route_is_bit_identical_to_the_real_time_route() {
    let stems = synth_song(RATE, 8.0, 7);
    let frames = stems[0][0].len();
    let mut rep = Report::new("prerender_bit_identical");
    for (semis, shift_drums, guide) in [(3, false, 0.0), (-5, false, 0.7), (7, true, 0.3)] {
        let p = MixParams { semitones: semis, shift_drums, guide, drums: 0.8, ..MixParams::default() };
        // Real-time route (the offline renderer runs the playback graph).
        let want = render_native(&stems, RATE, p, &[]).channels;
        let cache = pitched(&stems, p);
        let mut g = graph(p, false);
        g.set_auto_route(false);
        g.reset_cached(0, semis, shift_drums);
        let got = run(&stems, &mut g, Some((&cache, frames)), &mut |_, _| {});
        assert_eq!(first_diff(&got, &want, 0, frames), None, "key {semis:+} drums {shift_drums}: cached route differs");
        assert_eq!(g.stats().cache_misses, 0);
        assert_eq!(g.route().0, RouteInfo::Cached { slot: 0, semitones: semis });
        rep.line(format!(
            "key {semis:+}, shift drums {shift_drums}, guide {guide}: cached route == real-time route for all {frames} frames (bit-identical)"
        ));
    }
}

#[test]
fn switching_to_and_from_the_cached_key_uses_the_key_change_crossfade() {
    let stems = synth_song(RATE, 8.0, 11);
    let frames = stems[0][0].len();
    let p = MixParams { semitones: 4, ..MixParams::default() };
    let bypass = render_native(&stems, RATE, MixParams { semitones: 0, ..p }, &[]).channels;
    let shifted = render_native(&stems, RATE, p, &[]).channels;
    let cache = pitched(&stems, p);
    let mut g = graph(p, false);
    g.set_auto_route(false);
    g.reset(); // prerender policy: starts on the bypass even though the key is +4
    let (on, off) = (2 * RATE as u64 / BLOCK as u64 * BLOCK as u64, 5 * RATE as u64 / BLOCK as u64 * BLOCK as u64);
    let got = run(&stems, &mut g, Some((&cache, frames)), &mut |g, pos| {
        if pos == on {
            g.request_cached(0, 4, false);
        }
        if pos == off {
            g.request_bypass();
        }
    });
    let t = EqualPowerTable::new(g.crossfade_len());
    let len = t.len();
    let (on, off) = (on as usize, off as usize);
    let mut want = [bypass[0].clone(), bypass[1].clone()];
    for c in 0..2 {
        for f in on..off {
            want[c][f] = if f < on + len { bypass[c][f] * t.fade_out[f - on] + shifted[c][f] * t.fade_in[f - on] } else { shifted[c][f] };
        }
        for f in off..off + len {
            want[c][f] = shifted[c][f] * t.fade_out[f - off] + bypass[c][f] * t.fade_in[f - off];
        }
    }
    assert_eq!(first_diff(&got, &want, 0, frames), None, "output differs from bypass -> crossfade -> cached -> crossfade -> bypass");
    assert_eq!(g.stats().transitions, 2);
    assert_eq!(g.route().0, RouteInfo::Bypass);
    println!("prerender switch: bypass -> cached +4 at {on} and back at {off}: equal to the equal-power crossfade formula for all {frames} frames");
}

#[test]
fn a_missing_cache_block_fades_to_the_bypass_before_the_gap() {
    let stems = synth_song(RATE, 6.0, 3);
    let frames = stems[0][0].len();
    let p = MixParams { semitones: -3, ..MixParams::default() };
    let bypass = render_native(&stems, RATE, MixParams { semitones: 0, ..p }, &[]).channels;
    let shifted = render_native(&stems, RATE, p, &[]).channels;
    let cache = pitched(&stems, p);
    let gap = 3 * RATE as usize / BLOCK * BLOCK;
    let mut g = graph(p, false);
    g.set_auto_route(false);
    g.reset_cached(0, -3, false);
    let got = run(&stems, &mut g, Some((&cache, gap)), &mut |g, pos| {
        if pos as usize == gap {
            g.cache_miss(0);
        }
    });
    let t = EqualPowerTable::new(g.crossfade_len());
    let start = gap - t.len();
    let mut want = [shifted[0].clone(), shifted[1].clone()];
    for c in 0..2 {
        for f in start..frames {
            want[c][f] = if f < gap { shifted[c][f] * t.fade_out[f - start] + bypass[c][f] * t.fade_in[f - start] } else { bypass[c][f] };
        }
    }
    assert_eq!(first_diff(&got, &want, 0, frames), None, "the fade must end where the cached data ends");
    assert_eq!(g.stats().cache_misses, 1);
    println!("cache miss at frame {gap}: crossfade to the bypass over [{start}, {gap}), output equal to the formula");
}

#[test]
fn turning_the_real_time_policy_back_on_leaves_the_cached_route_by_crossfade() {
    let stems = synth_song(RATE, 6.0, 5);
    let frames = stems[0][0].len();
    let p = MixParams { semitones: 2, ..MixParams::default() };
    let cache = pitched(&stems, p);
    let mut g = graph(p, false);
    g.set_auto_route(false);
    g.reset_cached(0, 2, false);
    let back = 2 * RATE as u64 / BLOCK as u64 * BLOCK as u64;
    let got = run(&stems, &mut g, Some((&cache, frames)), &mut |g, pos| {
        if pos == back {
            g.set_auto_route(true);
        }
    });
    assert_eq!(g.route().0, RouteInfo::Shift { semitones: 2 });
    assert_eq!(g.stats().shift_shortfall_frames, 0);
    assert_eq!(g.stats().transitions, 1);
    // Click check: the largest sample step stays within the signal's own largest step.
    let step = |v: &[f32], a: usize, b: usize| (a + 1..b).map(|i| (v[i] - v[i - 1]).abs()).fold(0.0f32, f32::max);
    let b = back as usize;
    let around = step(&got[0], b - 2048, b + 8192);
    let elsewhere = step(&got[0], 4096, b - 4096);
    assert!(around <= elsewhere * 1.5, "step {around} at the switch vs {elsewhere} elsewhere");
    println!("cached -> real-time +2 at {b}: one crossfade, no shortfall, max step {around:.4} vs {elsewhere:.4} elsewhere");
}

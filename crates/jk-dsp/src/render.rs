// SPDX-License-Identifier: GPL-3.0-or-later
//! Offline render: the exact same graph as playback, single-threaded, for export, tests
//! and the Windows-vs-macOS comparison.
//!
//! Timing of parameter events (song frames): an event at frame `f` starts its gain ramp and
//! its key / drum-routing crossfade at output frame `f`.
//!
//! The output is exactly `frames` long at the song rate (latency removed), or
//! `round(frames * out_rate / song_rate)` frames when resampling.

use crate::graph::{DspError, GraphConfig, GraphStats, PitchGraph};
use crate::params::{MixParams, StemBlock, CHANNELS};
use crate::resample::OutputResampler;

/// Borrowed full-length stems: `stems[stem][channel]`, all `frames` long.
#[derive(Clone, Copy)]
pub struct StemsRef<'a> {
    pub sample_rate: u32,
    pub frames: usize,
    pub stems: [[&'a [f32]; CHANNELS]; 4],
}

/// A parameter change at a song frame.
#[derive(Clone, Copy, Debug)]
pub struct ParamEvent {
    pub frame: u64,
    pub params: MixParams,
}

#[derive(Clone, Copy, Debug)]
pub struct RenderOptions {
    /// Output rate. Equal to the song rate means no resampling.
    pub out_rate: u32,
    /// Master gain, applied last (1.0 = untouched, bit-exact).
    pub master_gain: f32,
    /// Graph block size (the result does not depend on it; see tests).
    pub block: usize,
}

impl RenderOptions {
    pub fn native(sample_rate: u32) -> Self {
        RenderOptions { out_rate: sample_rate, master_gain: 1.0, block: 512 }
    }
}

pub struct RenderOutput {
    pub sample_rate: u32,
    pub channels: [Vec<f32>; CHANNELS],
    pub stats: GraphStats,
    /// Graph latency that was removed (song frames).
    pub graph_latency: usize,
    /// Resampler delay that was removed (output frames, 0 without resampling).
    pub resampler_delay: usize,
}

/// Render the song. `events` must be sorted by frame; events at frame 0 are applied
/// before the first sample (a key at frame 0 starts without a crossfade).
pub fn render(
    stems: StemsRef,
    initial: MixParams,
    events: &[ParamEvent],
    opts: &RenderOptions,
) -> Result<RenderOutput, DspError> {
    for s in &stems.stems {
        for ch in s {
            if ch.len() < stems.frames {
                return Err(DspError::Invalid("stem shorter than `frames`"));
            }
        }
    }
    if events.windows(2).any(|w| w[0].frame > w[1].frame) {
        return Err(DspError::Invalid("events must be sorted by frame"));
    }
    let block = opts.block.clamp(1, 8192);
    let mut start = initial;
    let mut first_event = 0;
    while first_event < events.len() && events[first_event].frame == 0 {
        start = events[first_event].params;
        first_event += 1;
    }
    let mut cfg = GraphConfig::new(stems.sample_rate);
    cfg.max_block = block;
    let mut graph = PitchGraph::new(cfg, start)?;
    let lat = graph.latency() as u64;
    let frames = stems.frames as u64;
    let total = frames + lat;

    // An event at song frame f is applied when input frame f is next; the graph then
    // starts both the gain ramp and the key crossfade at output (song) frame f.
    let ev: Vec<(u64, MixParams)> = events[first_event..].iter().map(|e| (e.frame, e.params)).collect();
    let mut ei = 0usize;

    let zeros = vec![0.0f32; block];
    let mut song_out: [Vec<f32>; CHANNELS] = [Vec::with_capacity(stems.frames), Vec::with_capacity(stems.frames)];
    let mut ol = vec![0.0f32; block];
    let mut or = vec![0.0f32; block];
    let mut pos = 0u64;
    while pos < total {
        while ei < ev.len() && ev[ei].0 <= pos {
            graph.set_params(&ev[ei].1);
            ei += 1;
        }
        let mut end = (pos + block as u64).min(total);
        if pos < frames {
            end = end.min(frames);
        }
        if ei < ev.len() {
            end = end.min(ev[ei].0.max(pos + 1));
        }
        let n = (end - pos) as usize;
        let blk = if pos < frames {
            let p = pos as usize;
            let mut st: [[&[f32]; CHANNELS]; 4] = [[&zeros[..]; CHANNELS]; 4];
            for (s, stem) in stems.stems.iter().enumerate() {
                for c in 0..CHANNELS {
                    st[s][c] = &stem[c][p..p + n];
                }
            }
            StemBlock { stems: st }
        } else {
            StemBlock { stems: [[&zeros[..n]; CHANNELS]; 4] }
        };
        graph.process(&blk, n, [&mut ol[..n], &mut or[..n]]);
        // Output frame (pos + i) is song frame (pos + i - L).
        for i in 0..n {
            if pos + i as u64 >= lat {
                song_out[0].push(ol[i]);
                song_out[1].push(or[i]);
            }
        }
        pos = end;
    }
    debug_assert_eq!(song_out[0].len(), stems.frames);
    let stats = graph.stats();

    let (mut channels, rate, rs_delay) = if opts.out_rate == stems.sample_rate {
        (song_out, stems.sample_rate, 0)
    } else {
        let (c, d) = resample_whole(&song_out, stems.sample_rate, opts.out_rate)?;
        (c, opts.out_rate, d)
    };
    if opts.master_gain != 1.0 {
        for ch in channels.iter_mut() {
            ch.iter_mut().for_each(|v| *v *= opts.master_gain);
        }
    }
    Ok(RenderOutput {
        sample_rate: rate,
        channels,
        stats,
        graph_latency: lat as usize,
        resampler_delay: rs_delay,
    })
}

/// Resample a whole planar buffer with the playback resampler, removing its delay.
/// Output length: `round(len * out_rate / in_rate)`.
pub fn resample_whole(
    input: &[Vec<f32>; CHANNELS],
    in_rate: u32,
    out_rate: u32,
) -> Result<([Vec<f32>; CHANNELS], usize), DspError> {
    let chunk = 1024;
    let mut rs = OutputResampler::new(in_rate, out_rate, chunk)?;
    let delay = rs.output_delay();
    let len = input[0].len();
    let out_len = ((len as u128 * out_rate as u128 + in_rate as u128 / 2) / in_rate as u128) as usize;
    let mut out: [Vec<f32>; CHANNELS] = [Vec::with_capacity(out_len + delay), Vec::with_capacity(out_len + delay)];
    let mut l = vec![0.0f32; chunk];
    let mut r = vec![0.0f32; chunk];
    let mut pos = 0usize;
    while out[0].len() < out_len + delay {
        for i in 0..chunk {
            let k = pos + i;
            l[i] = if k < len { input[0][k] } else { 0.0 };
            r[i] = if k < len { input[1][k] } else { 0.0 };
        }
        let produced = rs.process([&l, &r])?;
        out[0].extend_from_slice(produced[0]);
        out[1].extend_from_slice(produced[1]);
        pos += chunk;
    }
    for ch in out.iter_mut() {
        ch.drain(..delay);
        ch.truncate(out_len);
    }
    Ok((out, delay))
}

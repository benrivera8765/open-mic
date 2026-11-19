// SPDX-License-Identifier: GPL-3.0-or-later
//! Shared by the tracker examples: `field=value` overrides of a `TrackerConfig`, for
//! exploring the settings (`tracker_eval --set`, `tracker_bench` with `JK_SET`). Not used
//! for the recorded default numbers.

use jk_pitch::tracker::TrackerConfig;

/// Applies one `field=value` override. Panics on an unknown field or a bad value.
pub fn set_field(cfg: &mut TrackerConfig, kv: &str) {
    let (k, v) = kv.split_once('=').expect("--set field=value");
    let f = || v.parse::<f64>().unwrap_or_else(|_| panic!("bad value {v}"));
    match k {
        "window_s" => cfg.window_s = f(),
        "hop_s" => cfg.hop_s = f(),
        "min_hz" => cfg.min_hz = f(),
        "max_hz" => cfg.max_hz = f(),
        "highpass_hz" => cfg.highpass_hz = f(),
        "lowpass_hz" => cfg.lowpass_hz = f(),
        "peak_threshold" => cfg.peak_threshold = f() as f32,
        "clarity_on" => cfg.clarity_on = f() as f32,
        "clarity_off" => cfg.clarity_off = f() as f32,
        "level_on_dbfs" => cfg.level_on_dbfs = f() as f32,
        "level_off_dbfs" => cfg.level_off_dbfs = f() as f32,
        "min_voiced_frames" => cfg.min_voiced_frames = f() as u32,
        "max_step_cents" => cfg.max_step_cents = f() as f32,
        "octave_cost" => cfg.octave_cost = f() as f32,
        "flat_power" => cfg.flat_power = f() as f32,
        "flat_max_hz" => cfg.flat_max_hz = f(),
        "candidate_floor" => cfg.candidate_floor = f() as f32,
        "jump_cost" => cfg.jump_cost = f() as f32,
        "jump_up_cost" => cfg.jump_up_cost = f() as f32,
        "lookahead_frames" => cfg.lookahead_frames = f() as u32,
        "clarity_steady" => cfg.clarity_steady = f() as f32,
        "steady_frames" => cfg.steady_frames = f() as u32,
        "steady_step_cents" => cfg.steady_step_cents = f() as f32,
        "steady_min_hz" => cfg.steady_min_hz = f(),
        other => panic!("unknown setting {other}"),
    }
}

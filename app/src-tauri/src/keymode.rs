// SPDX-License-Identifier: GPL-3.0-or-later
//! Key-change mode for this computer (WORK_ORDER 4.5, last bullet; DECISIONS: key-change
//! render-ahead cache).
//!
//! `audio.key_mode` is "auto" (default), "realtime" or "prerender". "auto" measures
//! Rubber Band R3's real-time factor once per computer and sample rate
//! (`jk_engine::benchmark_realtime_factor`, about 0.5 s on a fast PC, in the background)
//! and keeps the result in `audio.key_benchmarks["<computer>@<rate>"]`, so a stick moved
//! between a PC and a Mac measures each once. Below `jk_engine::AUTO_MIN_REALTIME_FACTOR`
//! (3x) the engine renders keys ahead into `<Settings>/cache/keys`; until a measurement
//! exists it renders ahead too (the safe side). The computer part of the key is a hash
//! (OS, CPU architecture, logical CPU count, and on Windows the computer name and CPU
//! identifier), not the computer's name.

use std::collections::BTreeMap;
use std::path::PathBuf;

use crate::portable::{DataLayout, DataMode};
use crate::settings::{AudioSettings, KeyModeSetting};
use jk_engine::KeyMode;

/// A short, stable, non-identifying id of this computer.
pub fn machine_id() -> String {
    #[cfg_attr(not(windows), allow(unused_mut))] // only Windows adds parts below
    let mut parts = vec![
        std::env::consts::OS.to_string(),
        std::env::consts::ARCH.to_string(),
        std::thread::available_parallelism().map_or(0, |n| n.get()).to_string(),
    ];
    #[cfg(windows)]
    for k in ["COMPUTERNAME", "PROCESSOR_IDENTIFIER"] {
        parts.push(std::env::var(k).unwrap_or_default());
    }
    // FNV-1a 64: stable across versions and platforms (std's hasher is not).
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in parts.join("|").bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{}-{}-{:012x}", std::env::consts::OS, std::env::consts::ARCH, h & 0xffff_ffff_ffff)
}

pub fn bench_key(machine: &str, rate: u32) -> String {
    format!("{machine}@{rate}")
}

/// This computer's measured factors by sample rate.
pub fn factors_for(machine: &str, benchmarks: &BTreeMap<String, f64>) -> BTreeMap<u32, f64> {
    let prefix = format!("{machine}@");
    benchmarks
        .iter()
        .filter_map(|(k, v)| k.strip_prefix(&prefix).and_then(|r| r.parse().ok()).map(|r| (r, *v)))
        .collect()
}

/// The mode for a song at `rate`.
pub fn resolve(setting: KeyModeSetting, factor: Option<f64>) -> KeyMode {
    match setting {
        KeyModeSetting::Realtime => KeyMode::Realtime,
        KeyModeSetting::Prerender => KeyMode::Prerender,
        KeyModeSetting::Auto => jk_engine::choose_key_mode(factor),
    }
}

/// `<Settings>/cache/keys`, next to the Studio's caches (D-153). None in the throw-away
/// macOS translocation folder (prerender then falls back to real time, with a note).
pub fn cache_dir(layout: &DataLayout) -> Option<PathBuf> {
    match layout.mode {
        DataMode::Translocated => None,
        _ => Some(layout.settings_dir.join("cache").join("keys")),
    }
}

pub fn cache_bytes(a: &AudioSettings) -> u64 {
    (a.key_cache_mb.clamp(64.0, 65536.0) * 1024.0 * 1024.0) as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mode_selection() {
        assert_eq!(resolve(KeyModeSetting::Realtime, Some(0.5)), KeyMode::Realtime);
        assert_eq!(resolve(KeyModeSetting::Prerender, Some(50.0)), KeyMode::Prerender);
        assert_eq!(resolve(KeyModeSetting::Auto, None), KeyMode::Prerender, "unmeasured: safe side");
        assert_eq!(resolve(KeyModeSetting::Auto, Some(2.5)), KeyMode::Prerender);
        assert_eq!(resolve(KeyModeSetting::Auto, Some(9.8)), KeyMode::Realtime);
    }

    #[test]
    fn benchmarks_are_per_computer_and_rate() {
        let me = machine_id();
        assert_eq!(me, machine_id(), "stable");
        assert!(!me.contains(&std::env::var("COMPUTERNAME").unwrap_or_else(|_| "\u{1}".into())), "no computer name in clear");
        let mut b = BTreeMap::new();
        b.insert(bench_key(&me, 48_000), 9.0);
        b.insert(bench_key(&me, 44_100), 10.0);
        b.insert(bench_key("macos-aarch64-000000000000", 48_000), 2.0);
        let f = factors_for(&me, &b);
        assert_eq!(f.len(), 2);
        assert_eq!(f[&48_000], 9.0);
        assert_eq!(f[&44_100], 10.0);
    }
}

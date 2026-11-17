// SPDX-License-Identifier: GPL-3.0-or-later
//! Shared helpers for the jk-dsp integration tests.
#![allow(dead_code)]

use jk_dsp::{render, MixParams, ParamEvent, RenderOptions, RenderOutput, StemsRef};
use std::io::Write;

/// Write measured numbers both to stdout and to
/// `<target>/tmp/jk-dsp-results/<name>.txt` (copied into docs/benchmarks/dsp.md).
pub struct Report {
    name: String,
    lines: Vec<String>,
}

impl Report {
    pub fn new(name: &str) -> Self {
        Report { name: name.to_string(), lines: Vec::new() }
    }
    pub fn line(&mut self, s: impl Into<String>) {
        let s = s.into();
        println!("{s}");
        self.lines.push(s);
    }
}

impl Drop for Report {
    fn drop(&mut self) {
        let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("jk-dsp-results");
        let _ = std::fs::create_dir_all(&dir);
        let profile = if cfg!(debug_assertions) { "debug" } else { "release" };
        if let Ok(mut f) = std::fs::File::create(dir.join(format!("{}-{profile}.txt", self.name))) {
            for l in &self.lines {
                let _ = writeln!(f, "{l}");
            }
        }
    }
}

/// Owned stems `[stem][channel]`.
pub type Stems = [[Vec<f32>; 2]; 4];

pub fn silent_stems(frames: usize) -> Stems {
    std::array::from_fn(|_| [vec![0.0; frames], vec![0.0; frames]])
}

pub fn stems_ref(stems: &Stems, rate: u32) -> StemsRef<'_> {
    let frames = stems[0][0].len();
    StemsRef {
        sample_rate: rate,
        frames,
        stems: std::array::from_fn(|s| [&stems[s][0][..], &stems[s][1][..]]),
    }
}

pub fn render_native(stems: &Stems, rate: u32, params: MixParams, events: &[ParamEvent]) -> RenderOutput {
    render(stems_ref(stems, rate), params, events, &RenderOptions::native(rate)).expect("render")
}

pub fn render_block(stems: &Stems, rate: u32, params: MixParams, events: &[ParamEvent], block: usize) -> RenderOutput {
    let mut o = RenderOptions::native(rate);
    o.block = block;
    render(stems_ref(stems, rate), params, events, &o).expect("render")
}

/// Little-endian f32 bytes of an interleaved stereo signal.
pub fn interleaved_le_bytes(ch: &[Vec<f32>; 2]) -> Vec<u8> {
    let mut v = Vec::with_capacity(ch[0].len() * 8);
    for i in 0..ch[0].len() {
        v.extend_from_slice(&ch[0][i].to_le_bytes());
        v.extend_from_slice(&ch[1][i].to_le_bytes());
    }
    v
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let d = Sha256::digest(bytes);
    d.iter().map(|b| format!("{b:02x}")).collect()
}

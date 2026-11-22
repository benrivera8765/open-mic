// SPDX-License-Identifier: GPL-3.0-or-later
//! A generated test song: a pure tone in the `other` stem (pitched), the three other
//! stems digital silence, and one line of a public-domain lyric ("Twinkle, twinkle,
//! little star", Jane Taylor, 1806) timed letter by letter.
//!
//! Used by the self-check (test tone at -60 dBFS, key-change pitch measurement, the
//! 3-second capture) and by the app's tests. Generated at run time, so nothing
//! copyrighted and nothing large is shipped.

use std::path::Path;

use serde_json::json;

/// What to generate.
#[derive(Clone, Copy, Debug)]
pub struct ToneSong {
    pub sample_rate: u32,
    pub seconds: f64,
    pub freq_hz: f64,
    /// Peak amplitude of the tone (linear, 0..1).
    pub amplitude: f64,
}

impl Default for ToneSong {
    fn default() -> Self {
        ToneSong {
            sample_rate: 48_000,
            seconds: 8.0,
            freq_hz: 440.0,
            amplitude: 0.25,
        }
    }
}

/// The lyric line, one word per entry.
pub const LYRIC: [&str; 4] = ["Twinkle,", "twinkle,", "little", "star"];

fn lyrics_json(seconds: f64) -> serde_json::Value {
    // Laid out in "units" (a letter is 1, a gap between words 2, the held last letter
    // 8), then scaled to fill 10 %..90 % of the song, so any length works.
    let letters: usize = LYRIC.iter().map(|w| w.chars().count()).sum();
    let units = (letters - 1) as f64 + 8.0 + 2.0 * (LYRIC.len() - 1) as f64;
    let unit = 0.8 * seconds / units;
    let mut t = 0.1 * seconds;
    let mut words = Vec::new();
    for (wi, w) in LYRIC.iter().enumerate() {
        let start = t;
        let letters: Vec<char> = w.chars().collect();
        let mut chars = Vec::new();
        for (ci, c) in letters.iter().enumerate() {
            let held = wi == LYRIC.len() - 1 && ci == letters.len() - 1;
            let end = t + unit * if held { 8.0 } else { 1.0 };
            chars.push(json!([c.to_string(), round(t), round(end)]));
            t = end;
        }
        words.push(json!({"start": round(start), "end": round(t), "chars": chars}));
        t += 2.0 * unit;
    }
    let first = words[0]["start"].clone();
    let last = words[words.len() - 1]["end"].clone();
    json!({
        "lyrics_version": 1, "language": "en", "source": "user_text", "checked": true,
        "alignment": {"method": "synthetic", "model": null, "anchored_lines": false},
        "lines": [{"start": first, "end": last, "words": words}]
    })
}

fn round(t: f64) -> f64 {
    (t * 1000.0).round() / 1000.0
}

/// Writes the song to `path` (atomically). Title "Test Tone", artist "Jukeoke",
/// key A major (440 Hz is an A).
pub fn write(path: &Path, spec: ToneSong) -> jk_format::Result<()> {
    let frames = (spec.seconds * spec.sample_rate as f64).round() as usize;
    let tone = jk_dsp::signals::sine(spec.freq_hz, spec.sample_rate, frames, spec.amplitude, 0.0);
    let scale = 8_388_608.0f64; // 2^23: 24-bit full scale
    let mut other = Vec::with_capacity(frames * 2);
    for &x in &tone {
        let v = (x as f64 * scale).round().clamp(-scale, scale - 1.0) as i32;
        other.push(v);
        other.push(v);
    }
    let silence = vec![0i32; frames * 2];
    let roles = ["lead_vocal", "backing_vocals", "drums", "other"];
    let stems: Vec<serde_json::Value> = roles
        .iter()
        .map(|r| {
            json!({"role": r, "chunk": format!("stem.{r}"), "codec": "flac", "bits": 24, "gain": 1.0,
                   "peak": if *r == "other" { spec.amplitude } else { 0.0 },
                   "in_karaoke_mix": *r != "lead_vocal", "pitched": *r != "drums"})
        })
        .collect();
    let manifest: jk_format::Manifest = serde_json::from_value(json!({
        "format": "jksong", "format_version": jk_format::FORMAT_VERSION,
        "title": "Test Tone", "artist": "Open Mic", "album": null,
        "sample_rate": spec.sample_rate, "channels": 2, "duration_frames": frames,
        "duration_seconds": frames as f64 / spec.sample_rate as f64,
        "cover": null, "lyrics": "lyrics",
        "analysis": {"key": {"tonic": "A", "mode": "major", "name": "A major", "confidence": 1.0}, "tempo_bpm": null},
        "preset": "full", "stems_sum_to_source": false, "stems": stems,
        "provenance": {"app": {"name": "Open Mic self-check", "version": env!("CARGO_PKG_VERSION")},
                       "created_utc": jk_format::time::utc_now_iso8601(), "resampling": [], "models": [],
                       "source": {"filename": "generated", "lossy": false}}
    }))
    .map_err(|e| jk_format::FormatError::InvalidInput(e.to_string()))?;
    let mut w = jk_format::SongWriter::new(manifest);
    w.add_json_chunk("lyrics", &lyrics_json(spec.seconds))?;
    for r in roles {
        let samples = if r == "other" { &other } else { &silence };
        let flac = jk_engine::encode_flac(samples, 2, 24, spec.sample_rate);
        w.add_chunk(format!("stem.{r}"), "audio/flac", jk_format::ChunkData::Bytes(flac))?;
    }
    w.write_atomic(path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_song_reads_back() {
        let dir = std::env::temp_dir().join(format!(
            "jukeoke-testsong-{}-{:08x}",
            std::process::id(),
            crate::fs_atomic::random_u32()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("Open Mic - Test Tone.jksong");
        write(&p, ToneSong { seconds: 3.0, ..Default::default() }).unwrap();
        let song = jk_format::SongFile::open(&p).unwrap();
        assert_eq!(song.manifest().duration_frames, 144_000);
        let lyrics = song.effective_lyrics().unwrap().unwrap();
        assert_eq!(lyrics.lines[0].text(), "Twinkle, twinkle, little star");
        assert!(lyrics.validate(3.0, "test").is_empty(), "{:?}", lyrics.validate(3.0, "test"));
        let report = jk_format::verify_file(&p, &jk_format::VerifyOptions::default());
        assert!(report.ok(), "{:?}", report.errors);
        std::fs::remove_dir_all(dir).unwrap();
    }
}

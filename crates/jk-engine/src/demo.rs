// SPDX-License-Identifier: GPL-3.0-or-later
//! Generated songs (no copyrighted material anywhere).
//!
//! * [`make_demo_song`]: the bundled demo, "Twinkle, Twinkle, Little Star" (words: Jane
//!   Taylor, 1806; melody: the traditional French tune, both public domain), ~40 s,
//!   44.1 kHz, four mono stems synthesised here: a sine-and-harmonics "voice" for the lead
//!   vocal, a quiet diatonic harmony a third below for the backing vocals (lines 3 to 6),
//!   a chord pad for "other", and a soft kick / snare / hat loop for the drums. An 8 s
//!   instrumental break sits in the middle. Every syllable is one note, so the letter
//!   timings are known **by construction** (see [`demo_lyrics`]); they are the ground
//!   truth for the lyrics-ticker test and the app's self-check song.
//! * [`make_synth_song`]: a long stereo song from `jk_dsp::signals::synth_song`
//!   (benchmarks: memory, long playback runs).
//!
//! Letter timing rule (each syllable sung over 90 % of its note, then a short gap):
//! onset consonants 60 ms each, coda consonants 60 ms each, extra vowels of the nucleus
//! 80 ms each, and the first vowel of the nucleus is held for the rest of the note.
//! Punctuation is a zero-length grapheme at the end of its word. Times are rounded to
//! the microsecond, and the audio is generated from the same numbers.
//!
//! The stems carry 16 significant bits on the 24-bit grid (every sample is a multiple of
//! 256; the FLAC encoder codes them as "wasted bits") to keep the committed fixture
//! small. Generation is deterministic (integer noise, `libm` sines).

use std::f64::consts::PI;
use std::io::BufWriter;
use std::path::{Path, PathBuf};

use jk_format::{
    Analysis, ChunkData, Grapheme, KeyInfo, Line, Lyrics, Manifest, SongWriter, StemInfo as FmtStem, UserState, Word,
};
use serde_json::{json, Map, Value};

use crate::api::EngineError;
use crate::flac_write::FlacEncoder;

pub const DEMO_RATE: u32 = 44_100;
pub const DEMO_BPM: f64 = 120.0;
/// Song length in seconds.
pub const DEMO_SECONDS: f64 = 40.0;
const BEAT: f64 = 60.0 / DEMO_BPM;
/// Beats before line 1 (intro).
const INTRO_BEATS: f64 = 4.0;
/// Instrumental break between line 4 and line 5, in beats (8 s).
const BREAK_BEATS: f64 = 16.0;
/// Part of each note that is sung.
const SUNG: f64 = 0.9;

/// The words, by syllable (`|` separates syllables of a word), one entry per line.
const LINES: [&str; 6] = [
    "Twin|kle, twin|kle, lit|tle star,",
    "How I won|der what you are!",
    "Up a|bove the world so high,",
    "Like a dia|mond in the sky.",
    "Twin|kle, twin|kle, lit|tle star,",
    "How I won|der what you are!",
];

/// The melody (MIDI notes) per line; the last note of every line lasts two beats.
const MELODY: [[u8; 7]; 6] = [
    [60, 60, 67, 67, 69, 69, 67],
    [65, 65, 64, 64, 62, 62, 60],
    [67, 67, 65, 65, 64, 64, 62],
    [67, 67, 65, 65, 64, 64, 62],
    [60, 60, 67, 67, 69, 69, 67],
    [65, 65, 64, 64, 62, 62, 60],
];

/// Chord roots per two beats for each line (pad): C, F or G.
const CHORDS: [[char; 4]; 6] = [
    ['C', 'C', 'F', 'C'],
    ['F', 'C', 'G', 'C'],
    ['C', 'F', 'C', 'G'],
    ['C', 'F', 'C', 'G'],
    ['C', 'C', 'F', 'C'],
    ['F', 'C', 'G', 'C'],
];

fn r6(t: f64) -> f64 {
    (t * 1e6).round() / 1e6
}

fn midi_hz(m: f64) -> f64 {
    440.0 * libm::pow(2.0, (m - 69.0) / 12.0)
}

/// Start time (s) of line `li`.
fn line_start(li: usize) -> f64 {
    let mut beats = INTRO_BEATS + 8.0 * li as f64;
    if li >= 4 {
        beats += BREAK_BEATS;
    }
    beats * BEAT
}

/// One sung note: start, sung end, MIDI note, line and syllable text.
struct Note {
    start: f64,
    sung_end: f64,
    midi: u8,
    line: usize,
}

fn notes() -> Vec<Note> {
    let mut v = Vec::new();
    for (li, mel) in MELODY.iter().enumerate() {
        let mut t = line_start(li);
        for (k, &m) in mel.iter().enumerate() {
            let dur = if k == 6 { 2.0 * BEAT } else { BEAT };
            v.push(Note { start: r6(t), sung_end: r6(t + SUNG * dur), midi: m, line: li });
            t += dur;
        }
    }
    v
}

fn is_vowel(c: char, first: bool, next_is_vowel: bool) -> bool {
    match c.to_ascii_lowercase() {
        'a' | 'e' | 'i' | 'o' | 'u' => true,
        // "y" is a vowel ("sky") unless it starts the syllable before a vowel ("you").
        'y' => !(first && next_is_vowel),
        _ => false,
    }
}

/// Letter timings of one syllable sung over `[start, end)`. Returns graphemes in order;
/// trailing punctuation gets zero length at `end`.
fn syllable_chars(syl: &str, start: f64, end: f64) -> Vec<Grapheme> {
    let chars: Vec<char> = syl.chars().collect();
    let letters: Vec<char> = chars.iter().copied().filter(|c| c.is_alphabetic()).collect();
    let punct: Vec<char> = chars.iter().copied().filter(|c| !c.is_alphabetic()).collect();
    let vowel: Vec<bool> = (0..letters.len())
        .map(|i| {
            let next = letters.get(i + 1).is_some_and(|&n| "aeiou".contains(n.to_ascii_lowercase()));
            is_vowel(letters[i], i == 0, next)
        })
        .collect();
    let first_v = vowel.iter().position(|&v| v).unwrap_or(0);
    let mut nuc_end = first_v;
    while nuc_end < letters.len() && vowel[nuc_end] {
        nuc_end += 1;
    }
    let nuc_end = nuc_end.max(first_v + 1);
    const CONS: f64 = 0.06;
    const EXTRA_VOWEL: f64 = 0.08;
    let onset = first_v as f64 * CONS;
    let coda = (letters.len() - nuc_end) as f64 * CONS;
    let extra = (nuc_end - first_v - 1) as f64 * EXTRA_VOWEL;
    let held = (end - start) - onset - coda - extra;
    assert!(held > 0.05, "syllable {syl:?} too short for its letters");
    let mut out = Vec::new();
    let mut t = start;
    for (i, &c) in letters.iter().enumerate() {
        let d = if i < first_v || i >= nuc_end {
            CONS
        } else if i == first_v {
            held
        } else {
            EXTRA_VOWEL
        };
        let (a, b) = (r6(t), if i + 1 == letters.len() { r6(end) } else { r6(t + d) });
        out.push(Grapheme::new(c.to_string(), a, b));
        t += d;
    }
    for c in punct {
        out.push(Grapheme::new(c.to_string(), r6(end), r6(end)));
    }
    out
}

/// The demo song's lyrics with their constructed timings (ground truth).
pub fn demo_lyrics() -> Lyrics {
    let notes = notes();
    let mut lines = Vec::new();
    let mut ni = 0;
    for (li, text) in LINES.iter().enumerate() {
        let mut words = Vec::new();
        for w in text.split(' ') {
            let mut chars = Vec::new();
            for syl in w.split('|') {
                let n = &notes[ni];
                debug_assert_eq!(n.line, li);
                chars.extend(syllable_chars(syl, n.start, n.sung_end));
                ni += 1;
            }
            let (s, e) = (chars.first().unwrap().start, chars.last().unwrap().end);
            words.push(Word { start: s, end: e, confidence: Some(1.0), chars, extra: Map::new() });
        }
        let (s, e) = (words.first().unwrap().start, words.last().unwrap().end);
        lines.push(Line { start: s, end: e, words, extra: Map::new() });
    }
    assert_eq!(ni, notes.len());
    Lyrics {
        lyrics_version: 1,
        language: Some("en".into()),
        source: Some("user_text".into()),
        checked: true,
        alignment: Some(json!({
            "method": "constructed",
            "note": "Generated song: every syllable is one note; letter times are exact by construction (jk-engine demo.rs)."
        })),
        lines,
        extra: Map::new(),
    }
}

/// Deterministic noise (xorshift32), uniform in [-1, 1).
struct Noise(u32);
impl Noise {
    fn next(&mut self) -> f64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.0 = x;
        (x as f64 / 2_147_483_648.0) - 1.0
    }
}

fn sin_cyc(cyc: f64) -> f64 {
    libm::sin(2.0 * PI * cyc.fract())
}

/// Attack / release envelope, exactly 0 at both ends.
fn env(t: f64, len: f64, attack: f64, release: f64) -> f64 {
    if t <= 0.0 || t >= len {
        return 0.0;
    }
    (t / attack).min(1.0) * ((len - t) / release).min(1.0)
}

/// A voice-like tone (fundamental + two harmonics) into `buf` over `[start, end)`.
fn add_tone(buf: &mut [f64], rate: f64, start: f64, end: f64, hz: f64, amp: f64, attack: f64) {
    let (a, b) = ((start * rate).round() as usize, ((end * rate).round() as usize).min(buf.len()));
    let len = end - start;
    for n in a..b {
        let t = (n - a) as f64 / rate;
        let e = env(t, len, attack, 0.04);
        if e == 0.0 {
            continue;
        }
        let c = hz * t;
        let y = sin_cyc(c) + 0.25 * libm::sin(2.0 * PI * (2.0 * c).fract() + 0.3) + 0.08 * libm::sin(2.0 * PI * (3.0 * c).fract() + 0.6);
        buf[n] += amp * e * y / 1.33;
    }
}

fn chord_notes(root: char) -> [f64; 3] {
    match root {
        'F' => [48.0, 53.0, 57.0], // C3 F3 A3
        'G' => [47.0, 50.0, 55.0], // B2 D3 G3
        _ => [48.0, 52.0, 55.0],   // C3 E3 G3
    }
}

/// Diatonic third below in C major.
fn third_below(m: u8) -> u8 {
    match m % 12 {
        0 => m - 3, // C -> A
        2 => m - 3, // D -> B
        4 => m - 4, // E -> C
        5 => m - 3, // F -> D
        7 => m - 3, // G -> E
        9 => m - 4, // A -> F
        _ => m - 3,
    }
}

/// The four demo stems (mono, f64), in role order lead, backing, drums, other.
fn demo_stems() -> [Vec<f64>; 4] {
    let rate = DEMO_RATE as f64;
    let frames = (DEMO_SECONDS * rate) as usize;
    let mut lead = vec![0.0; frames];
    let mut back = vec![0.0; frames];
    let mut drums = vec![0.0; frames];
    let mut pad = vec![0.0; frames];
    for n in notes() {
        add_tone(&mut lead, rate, n.start, n.sung_end, midi_hz(n.midi as f64), 0.30, 0.015);
        if n.line >= 2 {
            add_tone(&mut back, rate, n.start, n.sung_end, midi_hz(third_below(n.midi) as f64), 0.11, 0.03);
        }
    }
    // Pad: intro C, the lines' chords, the break (C F G C), outro C.
    let mut segs: Vec<(f64, f64, char)> = vec![(0.0, INTRO_BEATS * BEAT, 'C')];
    for li in 0..6 {
        for (k, &c) in CHORDS[li].iter().enumerate() {
            segs.push((line_start(li) + 2.0 * k as f64 * BEAT, 2.0 * BEAT, c));
        }
        if li == 3 {
            let b0 = line_start(3) + 8.0 * BEAT;
            for (k, c) in ['C', 'F', 'G', 'C'].iter().enumerate() {
                segs.push((b0 + 4.0 * k as f64 * BEAT, 4.0 * BEAT, *c));
            }
        }
    }
    let outro0 = line_start(5) + 8.0 * BEAT;
    segs.push((outro0, 8.0 * BEAT, 'C'));
    for (s, d, c) in segs {
        for m in chord_notes(c) {
            let hz = midi_hz(m);
            let (a, b) = ((s * rate).round() as usize, (((s + d) * rate).round() as usize).min(frames));
            for i in a..b {
                let t = (i - a) as f64 / rate;
                let e = env(t, d, 0.03, 0.03);
                pad[i] += 0.05 * e * (sin_cyc(hz * t) + 0.3 * libm::sin(2.0 * PI * (2.0 * hz * t).fract()));
            }
        }
    }
    // Drums: kick on beats 1 and 3, snare on 2 and 4, a hat on every off-beat, from the
    // start to the middle of the outro.
    let mut noise = Noise(0x1234_5678);
    let last_beat = ((outro0 + 4.0 * BEAT) / BEAT) as usize;
    for beat in 0..last_beat {
        let t0 = beat as f64 * BEAT;
        let a = (t0 * rate).round() as usize;
        if beat % 2 == 0 {
            // Kick: falling sine, 0.3 s.
            let len = 0.3;
            let mut ph = 0.0;
            for i in 0..(len * rate) as usize {
                let t = i as f64 / rate;
                let f = 45.0 + 75.0 * libm::exp(-t / 0.03);
                ph += f / rate;
                let e = libm::exp(-t / 0.12) * (1.0 - t / len);
                if a + i < frames {
                    drums[a + i] += 0.35 * e * sin_cyc(ph);
                }
            }
        } else {
            // Snare: noise + a 180 Hz body, 0.2 s.
            let len = 0.2;
            for i in 0..(len * rate) as usize {
                let t = i as f64 / rate;
                let taper = 1.0 - t / len;
                let y = 0.10 * libm::exp(-t / 0.05) * noise.next() + 0.08 * libm::exp(-t / 0.06) * sin_cyc(180.0 * t);
                if a + i < frames {
                    drums[a + i] += y * taper;
                }
            }
        }
        // Hat on the off-beat: noise, 50 ms.
        let h = ((t0 + BEAT / 2.0) * rate).round() as usize;
        let len = 0.05;
        for i in 0..(len * rate) as usize {
            let t = i as f64 / rate;
            if h + i < frames {
                drums[h + i] += 0.035 * libm::exp(-t / 0.012) * (1.0 - t / len) * noise.next();
            }
        }
    }
    [lead, back, drums, pad]
}

/// 16 significant bits on the 24-bit grid.
fn to_i24_16bit(x: f64) -> i32 {
    ((x * 32_768.0).round().clamp(-32_768.0, 32_767.0) as i32) * 256
}

/// A small generated PNG cover: a five-pointed star on a night-blue gradient.
fn demo_cover_png(size: u32) -> Vec<u8> {
    let mut raw = Vec::with_capacity((size * (size * 3 + 1)) as usize);
    let c = size as f64 / 2.0;
    for y in 0..size {
        raw.push(0); // filter: none
        for x in 0..size {
            let (dx, dy) = (x as f64 + 0.5 - c, y as f64 + 0.5 - c);
            let r = (dx * dx + dy * dy).sqrt() / c;
            let a = libm::atan2(dy, dx) + PI / 2.0;
            // Star radius as a function of angle (5 points).
            let k = ((a / (2.0 * PI / 5.0)).rem_euclid(1.0) - 0.5).abs() * 2.0;
            let star = 0.38 + 0.42 * (1.0 - k);
            let (mut rr, mut gg, mut bb) = (20.0 + 30.0 * (y as f64 / size as f64), 18.0, 60.0 + 50.0 * (y as f64 / size as f64));
            if r < star * 0.95 {
                (rr, gg, bb) = (255.0, 214.0, 90.0);
            } else if r < star {
                (rr, gg, bb) = (255.0, 160.0, 60.0);
            }
            raw.extend_from_slice(&[rr as u8, gg as u8, bb as u8]);
        }
    }
    let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::best());
    std::io::Write::write_all(&mut z, &raw).expect("in-memory");
    let idat = z.finish().expect("in-memory");
    let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
    let mut chunk = |ty: &[u8], data: &[u8]| {
        png.extend_from_slice(&(data.len() as u32).to_be_bytes());
        let mut h = crc32fast::Hasher::new();
        h.update(ty);
        h.update(data);
        png.extend_from_slice(ty);
        png.extend_from_slice(data);
        png.extend_from_slice(&h.finalize().to_be_bytes());
    };
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&size.to_be_bytes());
    ihdr.extend_from_slice(&size.to_be_bytes());
    ihdr.extend_from_slice(&[8, 2, 0, 0, 0]); // 8-bit RGB
    chunk(b"IHDR", &ihdr);
    chunk(b"IDAT", &idat);
    chunk(b"IEND", &[]);
    png
}

fn app_name() -> String {
    // branding.json is the only place the app name lives (docs/ARCHITECTURE.md 9).
    const BRANDING: &str = include_str!("../../../branding.json");
    serde_json::from_str::<Value>(BRANDING)
        .ok()
        .and_then(|v| v.get("name").and_then(Value::as_str).map(str::to_string))
        .unwrap_or_else(|| "app".into())
}

fn io_err(e: impl std::fmt::Display) -> EngineError {
    EngineError::new("write_failed", "The song file could not be written.", e.to_string())
}

/// What a generator wrote.
#[derive(Clone, Debug)]
pub struct MadeSong {
    pub path: PathBuf,
    pub bytes: u64,
    pub frames: u64,
    pub sample_rate: u32,
    pub stem_bytes: Vec<(String, u64)>,
}

/// Encode one stem (interleaved i32 on the 24-bit grid) to a temporary FLAC file.
fn encode_stem_file(tmp: &Path, samples: &[i32], channels: usize, rate: u32) -> Result<u64, EngineError> {
    let f = std::fs::File::create(tmp).map_err(io_err)?;
    let frames = (samples.len() / channels) as u64;
    let mut enc = FlacEncoder::new(BufWriter::with_capacity(1 << 20, f), channels, 24, rate, Some(frames)).map_err(io_err)?;
    for chunk in samples.chunks(65_536 * channels) {
        enc.write_interleaved(chunk).map_err(io_err)?;
    }
    let (w, s) = enc.finish().map_err(io_err)?;
    w.into_inner().map_err(|e| io_err(e.error()))?;
    Ok(s.bytes)
}

#[allow(clippy::too_many_arguments)]
fn write_song(
    out: &Path,
    title: &str,
    artist: &str,
    rate: u32,
    channels: u16,
    frames: u64,
    stems: &[(&str, Vec<i32>, f64, bool, bool)],
    lyrics: Option<Lyrics>,
    cover: Option<Vec<u8>>,
    key: Option<(&str, &str)>,
    tempo: Option<f64>,
    description: &str,
) -> Result<MadeSong, EngineError> {
    let tmp_dir = out.parent().map(Path::to_path_buf).unwrap_or_else(|| PathBuf::from("."));
    let mut stem_infos = Vec::new();
    let mut files = Vec::new();
    let mut stem_bytes = Vec::new();
    for (k, (role, samples, peak, in_mix, pitched)) in stems.iter().enumerate() {
        let tmp = tmp_dir.join(format!(".{}.stem{k}.tmp-flac", out.file_name().map(|n| n.to_string_lossy()).unwrap_or_default()));
        let bytes = encode_stem_file(&tmp, samples, channels as usize, rate)?;
        stem_bytes.push((role.to_string(), bytes));
        files.push((format!("stem.{role}"), tmp));
        stem_infos.push(FmtStem {
            role: role.to_string(),
            chunk: format!("stem.{role}"),
            codec: "flac".into(),
            bits: 24,
            gain: 1.0,
            peak: Some(*peak),
            in_karaoke_mix: Some(*in_mix),
            pitched: Some(*pitched),
            extra: Map::new(),
        });
    }
    let name = app_name();
    let manifest = Manifest {
        format: "jksong".into(),
        format_version: jk_format::FORMAT_VERSION.into(),
        title: title.into(),
        artist: artist.into(),
        album: None,
        sample_rate: rate,
        channels,
        duration_frames: frames,
        duration_seconds: frames as f64 / rate as f64,
        cover: cover.as_ref().map(|_| "cover".to_string()),
        lyrics: lyrics.as_ref().map(|_| "lyrics".to_string()),
        analysis: Some(Analysis {
            key: key.map(|(t, m)| KeyInfo {
                tonic: t.into(),
                mode: m.into(),
                name: Some(format!("{t} {m}")),
                confidence: Some(1.0),
                extra: Map::new(),
            }),
            tempo_bpm: tempo,
            extra: Map::new(),
        }),
        preset: Some("full".into()),
        stems_sum_to_source: true,
        stems: stem_infos,
        provenance: json!({
            "app": { "name": name, "version": env!("CARGO_PKG_VERSION") },
            "generator": { "tool": "jukeoke-cli", "description": description },
            "source": { "filename": null, "generated": true, "lossy": false },
            "models": [],
            "resampling": [],
            "created_utc": "2026-11-17T00:00:00Z"
        }),
        extra: Map::new(),
    };
    let mut w = SongWriter::new(manifest).user_state(UserState::default());
    let result = (|| -> Result<(), EngineError> {
        if let Some(l) = &lyrics {
            w.add_json_chunk("lyrics", l).map_err(|e| io_err(&e))?;
        }
        if let Some(c) = cover {
            w.add_chunk("cover", "image/png", ChunkData::Bytes(c)).map_err(|e| io_err(&e))?;
        }
        for (id, p) in &files {
            w.add_chunk(id.clone(), "audio/flac", ChunkData::File(p.clone())).map_err(|e| io_err(&e))?;
        }
        Ok(())
    })();
    let written = result.and_then(|_| w.write_atomic(out).map_err(|e| io_err(&e)));
    for (_, p) in &files {
        let _ = std::fs::remove_file(p);
    }
    written?;
    let bytes = std::fs::metadata(out).map_err(io_err)?.len();
    Ok(MadeSong { path: out.to_path_buf(), bytes, frames, sample_rate: rate, stem_bytes })
}

/// Write the demo song (see the module documentation) to `out`, atomically.
pub fn make_demo_song(out: &Path) -> Result<MadeSong, EngineError> {
    let stems = demo_stems();
    let frames = stems[0].len() as u64;
    let roles = [("lead_vocal", false, true), ("backing_vocals", true, true), ("drums", true, false), ("other", true, true)];
    let mut list = Vec::new();
    for (k, (role, in_mix, pitched)) in roles.iter().enumerate() {
        let ints: Vec<i32> = stems[k].iter().map(|&x| to_i24_16bit(x)).collect();
        let peak = ints.iter().map(|&v| (v as f64 / 8_388_608.0).abs()).fold(0.0, f64::max);
        if peak >= 0.99 {
            return Err(EngineError::internal(format!("demo stem {role} peaks at {peak}")));
        }
        list.push((*role, ints, peak, *in_mix, *pitched));
    }
    write_song(
        out,
        "Twinkle, Twinkle, Little Star",
        "Traditional (words: Jane Taylor, 1806)",
        DEMO_RATE,
        1,
        frames,
        &list,
        Some(demo_lyrics()),
        Some(demo_cover_png(96)),
        Some(("C", "major")),
        Some(DEMO_BPM),
        "make-demo-song: generated demo song (public-domain words and melody, synthesised audio)",
    )
}

/// A test song whose lead vocal is one steady sine at `f0` Hz (amplitude 0.3, both
/// channels) and whose other stems are silent (no lyrics), 24-bit stereo. With the guide
/// vocal up, the pitch of the output is `f0` shifted by exactly the key heard, which makes
/// the moment of a key change measurable in played audio (Work Order 2, 4.6).
pub fn make_tone_song(out: &Path, seconds: f64, rate: u32, f0: f64) -> Result<MadeSong, EngineError> {
    let frames = (seconds * rate as f64).round() as usize;
    let tone = jk_dsp::signals::sine(f0, rate, frames, 0.3, 0.3);
    let mut lead = Vec::with_capacity(frames * 2);
    let mut peak = 0.0f64;
    for &x in &tone {
        let v = (x as f64 * 8_388_608.0).round().clamp(-8_388_608.0, 8_388_607.0) as i32;
        peak = peak.max((v as f64 / 8_388_608.0).abs());
        lead.push(v);
        lead.push(v);
    }
    let silent = vec![0i32; frames * 2];
    let list = vec![
        ("lead_vocal", lead, peak, false, true),
        ("backing_vocals", silent.clone(), 0.0, true, true),
        ("drums", silent.clone(), 0.0, true, false),
        ("other", silent, 0.0, true, true),
    ];
    write_song(
        out,
        &format!("Test tone {f0} Hz ({seconds:.0} s)"),
        "Generated",
        rate,
        2,
        frames as u64,
        &list,
        None,
        None,
        None,
        None,
        &format!("make_tone_song: steady {f0} Hz sine in the lead stem"),
    )
}

/// A long stereo test song from `jk_dsp::signals::synth_song` (no lyrics), 24-bit.
pub fn make_synth_song(out: &Path, seconds: f64, rate: u32, seed: u32) -> Result<MadeSong, EngineError> {
    let stems = jk_dsp::signals::synth_song(rate, seconds, seed);
    let frames = stems[0][0].len() as u64;
    let roles = [("lead_vocal", false, true), ("backing_vocals", true, true), ("drums", true, false), ("other", true, true)];
    let mut list = Vec::new();
    for (k, (role, in_mix, pitched)) in roles.iter().enumerate() {
        let mut ints = Vec::with_capacity(frames as usize * 2);
        let mut peak = 0.0f64;
        for i in 0..frames as usize {
            for c in 0..2 {
                let x = stems[k][c][i] as f64;
                peak = peak.max(x.abs());
                ints.push((x * 8_388_608.0).round().clamp(-8_388_608.0, 8_388_607.0) as i32);
            }
        }
        if peak >= 0.99 {
            return Err(EngineError::internal(format!("synth stem {role} peaks at {peak}")));
        }
        list.push((*role, ints, peak, *in_mix, *pitched));
    }
    drop(stems);
    write_song(
        out,
        &format!("Synthetic test song ({seconds:.0} s)"),
        "Generated",
        rate,
        2,
        frames,
        &list,
        None,
        None,
        None,
        None,
        &format!("make-test-song: jk_dsp::signals::synth_song seed {seed}"),
    )
}

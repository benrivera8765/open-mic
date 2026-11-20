// SPDX-License-Identifier: GPL-3.0-or-later
//! `jukeoke-cli practice ...`: the offline harness of practice mode (Work Order 2 section
//! 9, rows "Scoring", "Key change", "The owner's rig"). It plays a performance through the
//! very analysis chain the app runs (`jk_pitch::practice::Chain`: channel pick, echo
//! canceller, gate, tracker, song time, scorer) as if it were the microphone, with the
//! app's playback rendered offline by the engine's graph (jk-dsp, the export path) at the
//! same key(s) as the echo reference.
//!
//! * `practice run SONG (PERF.wav | --singer SPEC | --vocal) [options]`: one performance;
//!   prints (or writes) the result JSON. Scripted singers are generated from the song's
//!   own guide with the tracker's synthetic voice (`jk_pitch::tracker::testvoice`).
//! * `practice sweep SONG --keys -12:12 --wrong 1,-1,2,5`: the original isolated lead vocal
//!   rendered at every key and graded against the guide at that key, and against guides
//!   shifted by a different amount.
//!
//! Never prints lyrics: line rows are indices and times. Generated signals only; nothing
//! here opens a microphone or plays a sound.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

use clap::{Args, Subcommand};
use jk_dsp::{render, ParamEvent, RenderOptions, StemsRef};
use jk_engine::MixParams;
use jk_pitch::echo::testpath::{self, ReturnPath};
use jk_pitch::practice::{Chain, ChainConfig, ClockAnchor, LatencyConfig, PracticeLive};
use jk_pitch::score::{
    FinishContext, KeyTimeline, LineBasis, LineSpan, OctaveMode, ScoreConfig, ScoreGuide, ScoreNote, ScoreResult,
    Scorer, Strictness,
};
use jk_pitch::tracker::rig::{self, Compressor};
use jk_pitch::tracker::testvoice::{self as tv, Voice, Vowel};
use serde_json::{json, Value};

type Res<T> = Result<T, Box<dyn std::error::Error>>;

#[derive(Subcommand)]
pub enum PracticeCmd {
    /// Grade one performance through the practice analysis chain; result JSON.
    Run(Box<RunArgs>),
    /// The original lead vocal at every key against the guide at that key, and against
    /// wrongly shifted guides.
    Sweep(Box<SweepArgs>),
}

#[derive(Args, Clone)]
pub struct RunArgs {
    pub song: PathBuf,
    /// A performance WAV of generated sound (any rate, any channels).
    pub performance: Option<PathBuf>,
    /// A scripted singer from the guide: perfect | steady | sharp:C | flat:C | octave:N |
    /// late:MS | early:MS | one-note | silent | noise | rubato:MS[:SEED] | behind[:EARLY_MS]
    /// | ahead[:LATE_MS] | swap | staccato:SHARE | gap:SHARE:silence|breath|whisper|speech.
    #[arg(long)]
    pub singer: Option<String>,
    /// The song's own isolated lead vocal, rendered by the engine's graph at the key(s).
    #[arg(long)]
    pub vocal: bool,
    /// Key heard from the start (default: the song's saved key).
    #[arg(long, allow_hyphen_values = true)]
    pub key: Option<i32>,
    /// Key changes asked at song times, `t:k,...` (heard from the crossfade's midpoint).
    #[arg(long, allow_hyphen_values = true)]
    pub key_changes: Option<String>,
    /// The engine's published key timeline (`keys.json` from `key-switch-capture`), for a
    /// performance captured from the real engine; the WAV's first frame is song time
    /// `start_s` + `first_frame_index` of that file unless --start says otherwise.
    #[arg(long)]
    pub key_timeline: Option<PathBuf>,
    /// With --key-timeline: grade against the keys as they were ASKED for (request times),
    /// not as the engine says they were heard (comparison only).
    #[arg(long)]
    pub requested_keys: bool,
    /// Place the performance in the song by an exact match with the key-0 lead vocal render
    /// (an engine capture that starts in silence).
    #[arg(long)]
    pub align_to_vocal: bool,
    /// Grade against keys moved by this many semitones (a "wrongly shifted guide").
    #[arg(long, allow_hyphen_values = true, default_value_t = 0)]
    pub target_offset: i32,
    /// `auto`, `strict`, or a fixed shift (-2..2). Default: the song's saved choice.
    #[arg(long, allow_hyphen_values = true)]
    pub octave: Option<String>,
    /// relaxed | standard | strict (default: the song's, else standard).
    #[arg(long)]
    pub strictness: Option<String>,
    /// Music fed back into the input: `LEVEL_DB,DELAY_MS,KIND` with KIND usb | usb_eq |
    /// cable | bluetooth.
    #[arg(long, allow_hyphen_values = true)]
    pub feedback: Option<String>,
    /// Guide vocal level in the app's playback (what a mixer would return), 0..1. Default:
    /// the song's saved level.
    #[arg(long)]
    pub guide_vocal: Option<f32>,
    /// Delay of the voice (ms), as a hidden device delay would add it. Default: the
    /// feedback delay (the singer hears the song by the same path), else 0.
    #[arg(long, allow_hyphen_values = true)]
    pub voice_delay_ms: Option<f64>,
    /// Input channels, and the one carrying the voice (0-based).
    #[arg(long, default_value_t = 1)]
    pub channels: usize,
    #[arg(long, default_value_t = 0)]
    pub channel: usize,
    /// What the other channels carry: silent | noise | music.
    #[arg(long, default_value = "silent")]
    pub other: String,
    /// Input sample rate (default: 48000 for generated singers, else the file's / song's).
    #[arg(long)]
    pub rate: Option<u32>,
    /// Mixer reverb on the voice: `RT60_S[,WET_DB]`.
    #[arg(long, allow_hyphen_values = true)]
    pub reverb: Option<String>,
    /// Mixer compressor on the voice: moderate | heavy.
    #[arg(long)]
    pub compress: Option<String>,
    /// Singer level, dBFS RMS of the held notes.
    #[arg(long, allow_hyphen_values = true, default_value_t = -20.0)]
    pub level_db: f64,
    /// Vowel of the synthetic singer: ah | ee | oo.
    #[arg(long, default_value = "ah")]
    pub vowel: String,
    /// Manual latency offset (ms), used when nothing is measured.
    #[arg(long, allow_hyphen_values = true, default_value_t = 0.0)]
    pub latency_ms: f64,
    /// Do not use a measured delay (practice.auto_latency off).
    #[arg(long)]
    pub no_auto_latency: bool,
    /// Song time where the take starts and how long it runs (default: the whole song).
    #[arg(long, default_value_t = 0.0)]
    pub start: f64,
    #[arg(long)]
    pub seconds: Option<f64>,
    /// No echo stage (no reference rendered): faster, for sweeps without feedback.
    #[arg(long)]
    pub no_reference: bool,
    /// Input block size in frames (default: 10 ms).
    #[arg(long)]
    pub block: Option<usize>,
    /// Random jitter of the block time stamps, microseconds.
    #[arg(long, default_value_t = 0.0)]
    pub jitter_us: f64,
    /// Include every note's outcome in the result.
    #[arg(long)]
    pub notes: bool,
    /// Include the music-in-the-input status every half second.
    #[arg(long)]
    pub trace: bool,
    /// Scorer: bridge tracker dropouts up to this long on each side (ms; default: the
    /// scorer's).
    #[arg(long)]
    pub bridge_ms: Option<f64>,
    #[command(flatten)]
    pub scorer: ScorerOpts,
    /// Write the generated input (generated sound only) to this WAV.
    #[arg(long)]
    pub write_input: Option<PathBuf>,
    /// Diagnostics: write the tracker values the scorer received (song time, MIDI or null,
    /// level, clarity, flags) and the guide's pitch curve to this JSON (pitch numbers only,
    /// never lyrics; the input is generated or rendered sound, never a microphone).
    #[arg(long)]
    pub dump_samples: Option<PathBuf>,
    /// Write the result JSON here instead of stdout.
    #[arg(long)]
    pub out: Option<PathBuf>,
}

#[derive(Args, Clone)]
pub struct SweepArgs {
    pub song: PathBuf,
    /// Keys `from:to`.
    #[arg(long, default_value = "-12:12", allow_hyphen_values = true)]
    pub keys: String,
    /// Wrong guide shifts to grade each key's vocal against (never a multiple of 12).
    #[arg(long, default_value = "1,-1,2,-2,5", allow_hyphen_values = true)]
    pub wrong: String,
    /// Also render the karaoke mix at each key as the echo reference (slower).
    #[arg(long)]
    pub with_reference: bool,
    /// Worker threads (default: available parallelism).
    #[arg(long)]
    pub threads: Option<usize>,
    /// Scorer: bridge tracker dropouts up to this long on each side (ms).
    #[arg(long)]
    pub bridge_ms: Option<f64>,
    #[command(flatten)]
    pub scorer: ScorerOpts,
    /// Input rate (default: the song's rate).
    #[arg(long)]
    pub rate: Option<u32>,
    #[arg(long)]
    pub out: Option<PathBuf>,
}

/// Scorer settings for experiments (default: the scorer's own).
#[derive(Args, Clone, Default)]
pub struct ScorerOpts {
    /// Excusal rule: `SHARE[,CLARITY]` (share of a note's moments; lowest tracker clarity).
    /// `0` turns it off.
    #[arg(long)]
    pub excuse: Option<String>,
    /// Shortest rendition of a note the timing tolerance stretches onto it (share, 0..1).
    #[arg(long)]
    pub min_rendition: Option<f64>,
    /// Overlap allowed between renditions of consecutive graded notes (ms).
    #[arg(long)]
    pub overlap_ms: Option<f64>,
}

impl ScorerOpts {
    fn apply(&self, mut c: ScoreConfig) -> Res<ScoreConfig> {
        if let Some(e) = &self.excuse {
            let mut it = e.split(',');
            c.excuse_share = it.next().unwrap_or("0").trim().parse()?;
            if let Some(cl) = it.next() {
                c.excuse_clarity = cl.trim().parse()?;
            }
        }
        if let Some(m) = self.min_rendition {
            c.min_rendition = m;
        }
        if let Some(o) = self.overlap_ms {
            c.overlap_s = o / 1000.0;
        }
        Ok(c)
    }
}

pub fn run(cmd: PracticeCmd) -> Res<()> {
    match cmd {
        PracticeCmd::Run(a) => cmd_run(&a),
        PracticeCmd::Sweep(a) => cmd_sweep(&a),
    }
}

// ---------------------------------------------------------------------------------------
// The song: stems, guide, saved choices

/// What the harness needs from a song file.
struct Song {
    rate: u32,
    frames: usize,
    /// Planar stems in jk-dsp's order (lead, backing, drums, other).
    stems: [[Vec<f32>; 2]; 4],
    guide: ScoreGuide,
    excluded: Vec<(f64, f64)>,
    saved: MixParams,
    octave_mode: OctaveMode,
    octave_shift: i32,
    strictness: Option<Strictness>,
    name: String,
}

fn load_song(path: &Path, with_audio: bool) -> Res<Song> {
    let f = jk_format::SongFile::open(path)?;
    let rate = f.manifest().sample_rate;
    let saved = MixParams::from_settings(f.effective_settings());
    let (doc, _curve) = jk_format::guide::read_guide(&f)?.ok_or("this song has no melody guide (jukeoke-cli guide add)")?;
    let (state, _) = jk_guide::file_state(&f);
    let outdated = matches!(state, jk_format::GuideFileState::Outdated);
    let lyrics = f.effective_lyrics()?;
    let offset_s = f.effective_settings().lyrics_offset_ms as f64 / 1000.0;
    let refs_ok = doc.word_refs_valid(lyrics.as_ref(), f.effective_settings().lyrics_offset_ms);
    let notes: Vec<ScoreNote> = doc
        .notes
        .iter()
        .map(|n| ScoreNote {
            start: n.start,
            end: n.end,
            midi: n.midi,
            pitched: n.is_pitched(),
            confidence: n.confidence,
            octave_flagged: n.octave_flagged(),
            glide_in: n.glide_in,
            line: if refs_ok { n.word.as_ref().map(|w| w.0 as i64) } else { None },
        })
        .collect();
    let (lines, basis) = match &lyrics {
        Some(l) if !l.lines.is_empty() => (
            l.lines
                .iter()
                .enumerate()
                .map(|(i, line)| LineSpan { line: i as i64, start: line.start + offset_s, end: line.end + offset_s })
                .collect(),
            LineBasis::Lyrics,
        ),
        _ => (ScoreGuide::phrases(&notes), LineBasis::Phrases),
    };
    let practice = f.user_state().practice();
    let excluded = practice.as_ref().map(|p| p.excluded.iter().map(|e| (e.start, e.end)).collect()).unwrap_or_default();
    let (octave_mode, octave_shift) = practice
        .as_ref()
        .map(|p| (OctaveMode::parse(&p.octave.mode).unwrap_or(OctaveMode::Auto), p.octave.shift))
        .unwrap_or((OctaveMode::Auto, 0));
    let strictness = practice.as_ref().and_then(|p| p.strictness.as_deref().and_then(Strictness::parse));
    let frames_meta = f.manifest().duration_frames;
    let name = path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
    drop(f);
    let (frames, stems) = if with_audio { decode_stems(path)? } else { (frames_meta as usize, Default::default()) };
    let duration_s = frames as f64 / f64::from(rate);
    let guide = ScoreGuide {
        notes,
        low_confidence: doc.low_confidence,
        lines,
        basis,
        outdated,
        duration_s,
        label: format!("{} {}", doc.provenance.method, doc.provenance.version),
    };
    Ok(Song { rate, frames, stems, guide, excluded, saved, octave_mode, octave_shift, strictness, name })
}

/// Every stem of the song, decoded into memory by the engine's own reader (stored gains
/// applied, roles routed to the graph's slots).
fn decode_stems(path: &Path) -> Res<(usize, [[Vec<f32>; 2]; 4])> {
    let mut src = jk_engine::source::SongSource::open(path)?;
    let frames = src.info().frames as usize;
    let chunk = 1 << 16;
    let mut bufs = jk_engine::source::slot_buffers(chunk);
    let mut out: [[Vec<f32>; 2]; 4] = std::array::from_fn(|_| [Vec::with_capacity(frames), Vec::with_capacity(frames)]);
    let mut done = 0usize;
    while done < frames {
        let n = chunk.min(frames - done);
        src.read(&mut bufs, n)?;
        for s in 0..4 {
            for c in 0..2 {
                out[s][c].extend_from_slice(&bufs[s][c][..n]);
            }
        }
        done += n;
    }
    Ok((frames, out))
}

// ---------------------------------------------------------------------------------------
// Keys

/// The crossfade length of a key change in song frames (the switch is heard from its
/// midpoint, D-239).
fn crossfade_frames(rate: u32) -> Res<usize> {
    let g = jk_dsp::PitchGraph::new(jk_dsp::GraphConfig::new(rate), jk_dsp::MixParams::default())?;
    Ok(g.crossfade_len())
}

/// Key changes `t:k,...`.
fn parse_changes(s: &str) -> Res<Vec<(f64, i32)>> {
    s.split(',')
        .filter(|x| !x.trim().is_empty())
        .map(|x| {
            let (t, k) = x.split_once(':').ok_or("key changes: expected seconds:key")?;
            Ok((t.trim().parse()?, k.trim().parse()?))
        })
        .collect()
}

/// Render events for the key plan and the timeline of the key heard (segment 0).
fn key_plan(rate: u32, base: jk_dsp::MixParams, key0: i32, changes: &[(f64, i32)]) -> Res<(Vec<ParamEvent>, KeyTimeline)> {
    let half = crossfade_frames(rate)? as f64 / 2.0;
    let mut tl = KeyTimeline::default();
    tl.start_segment(0, 0.0, key0);
    let mut ev = Vec::new();
    for &(t, k) in changes {
        let frame = (t * f64::from(rate)).round().max(0.0) as u64;
        ev.push(ParamEvent { frame, params: jk_dsp::MixParams { semitones: k, ..base } });
        tl.change(0, (frame as f64 + half) / f64::from(rate), k);
    }
    ev.sort_by_key(|e| e.frame);
    Ok((ev, tl))
}

/// The key timeline the engine published (`keys.json` of `key-switch-capture`) and the
/// song time of the capture's first frame.
fn engine_timeline(path: &Path, from_requests: bool) -> Res<(KeyTimeline, f64)> {
    let v: Value = serde_json::from_slice(&std::fs::read(path)?)?;
    let rate = v["rate"].as_f64().ok_or("keys.json: rate")?;
    let start = v["start_s"].as_f64().unwrap_or(0.0);
    let k0 = v["first_frame_index"].as_f64().unwrap_or(0.0);
    let mut keys: Vec<Value> = v["keys"].as_array().ok_or("keys.json: keys")?.clone();
    if from_requests {
        // The key ASKED for, from the moment it was asked (the wrong timeline WO2 trap 1
        // warns about), for comparison.
        keys.truncate(1);
        for r in v["requests"].as_array().ok_or("keys.json: requests")? {
            keys.push(json!([r["position_s"], r["key"]]));
        }
    }
    let mut tl = KeyTimeline::default();
    for (i, e) in keys.iter().enumerate() {
        let t = e[0].as_f64().ok_or("keys.json: time")?;
        let k = e[1].as_i64().ok_or("keys.json: key")? as i32;
        if i == 0 {
            tl.start_segment(0, t, k);
        } else {
            tl.change(0, t, k);
        }
    }
    // Frame 0 of the capture is song time `start_s` (the capture starts when playback
    // starts; confirmed by an exact sample match on the three CC captures). `rate` and
    // `first_frame_index` are only checked to exist.
    let _ = (rate, k0);
    Ok((tl, start))
}

// ---------------------------------------------------------------------------------------
// Rendering

/// Renders the song with `params` and the key events, at `out_rate`.
fn render_mix(song: &Song, params: jk_dsp::MixParams, events: &[ParamEvent], out_rate: u32) -> Res<[Vec<f32>; 2]> {
    let st = StemsRef {
        sample_rate: song.rate,
        frames: song.frames,
        stems: std::array::from_fn(|s| [&song.stems[s][0][..], &song.stems[s][1][..]]),
    };
    let opts = RenderOptions { out_rate, ..RenderOptions::native(song.rate) };
    let out = render(st, params, events, &opts)?;
    Ok(out.channels)
}

/// The lead vocal alone (guide vocal 1, everything else off): what the original singer
/// sounds like at the key(s).
fn vocal_params(key: i32) -> jk_dsp::MixParams {
    jk_dsp::MixParams { guide: 1.0, backing_on: false, backing: 0.0, other: 0.0, drums: 0.0, shift_drums: false, semitones: key }
}

fn mono(st: &[Vec<f32>; 2]) -> Vec<f32> {
    st[0].iter().zip(&st[1]).map(|(a, b)| 0.5 * (a + b)).collect()
}

fn interleave2(st: &[Vec<f32>; 2]) -> Vec<f32> {
    let mut v = Vec::with_capacity(2 * st[0].len());
    for i in 0..st[0].len() {
        v.push(st[0][i]);
        v.push(st[1][i]);
    }
    v
}

/// Resamples planar channels with the playback resampler.
fn resample(chans: Vec<Vec<f32>>, from: u32, to: u32) -> Res<Vec<Vec<f32>>> {
    if from == to {
        return Ok(chans);
    }
    let mut out = Vec::new();
    for pair in chans.chunks(2) {
        let a = pair[0].clone();
        let b = pair.get(1).cloned().unwrap_or_else(|| vec![0.0; a.len()]);
        let (r, _) = jk_dsp::resample_whole(&[a, b], from, to)?;
        let [l, rr] = r;
        out.push(l);
        if pair.len() == 2 {
            out.push(rr);
        }
    }
    Ok(out)
}

fn read_wav(path: &Path) -> Res<(u32, Vec<Vec<f32>>)> {
    let mut r = hound::WavReader::open(path)?;
    let spec = r.spec();
    let ch = spec.channels as usize;
    let samples: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Float => r.samples::<f32>().collect::<Result<_, _>>()?,
        hound::SampleFormat::Int => {
            let scale = 1.0 / (1u64 << (spec.bits_per_sample - 1)) as f32;
            r.samples::<i32>().map(|s| s.map(|v| v as f32 * scale)).collect::<Result<_, _>>()?
        }
    };
    let frames = samples.len() / ch;
    let chans = (0..ch).map(|c| (0..frames).map(|i| samples[i * ch + c]).collect()).collect();
    Ok((spec.sample_rate, chans))
}

fn write_wav(path: &Path, rate: u32, interleaved: &[f32], channels: usize) -> Res<()> {
    let spec = hound::WavSpec { channels: channels as u16, sample_rate: rate, bits_per_sample: 32, sample_format: hound::SampleFormat::Float };
    let mut w = hound::WavWriter::create(path, spec)?;
    for &s in interleaved {
        w.write_sample(s)?;
    }
    w.finalize()?;
    Ok(())
}

// ---------------------------------------------------------------------------------------
// Scripted singers

#[derive(Clone, Copy, Debug)]
enum SingerKind {
    /// The guide's notes with a gentle vibrato (+-30 cents, 5.5 Hz) and a short scoop
    /// (60 ms from 80 cents below), off by `cents`, shifted by `octave`, `late` s late.
    Natural { cents: f64, octave: i32, late: f64 },
    /// The guide's notes held dead steady.
    Steady,
    /// Rubato: the natural singer with each note's onset moved by its own random offset in
    /// +-`range` s (uniform, seeded); a note is held until the next one starts, and keeps
    /// at least half of its length (the next onset waits for it).
    Rubato { range: f64, seed: u64 },
    /// Borrowing adversaries, never on the right note: each note's time sung at the pitch
    /// of the previous pitched note (`Behind`, moved `shift` s, negative = early), of the
    /// next one (`Ahead`, moved `shift` s), or adjacent notes swapped in pairs (`Swap`).
    Behind { shift: f64 },
    Ahead { shift: f64 },
    Swap,
    /// The natural singer sounding only the first `share` of every note (silent after).
    Staccato { share: f64 },
    /// The natural singer with the middle `share` of every note replaced by unvoiced sound
    /// at the same level (0 silence, 1 breath, 2 whisper-like fricatives, 3 speech-shaped
    /// noise): the excusal rule must not excuse it.
    Gap { share: f64, kind: u8 },
    /// One pitch whenever the guide sings.
    OneNote,
    Silent,
    Noise,
}

fn parse_singer(s: &str) -> Res<SingerKind> {
    let (name, arg) = s.split_once(':').map(|(a, b)| (a, Some(b))).unwrap_or((s, None));
    let num = |d: f64| -> Res<f64> { Ok(arg.map(str::parse::<f64>).transpose()?.unwrap_or(d)) };
    Ok(match name {
        "perfect" => SingerKind::Natural { cents: 0.0, octave: 0, late: 0.0 },
        "steady" => SingerKind::Steady,
        "sharp" => SingerKind::Natural { cents: num(30.0)?, octave: 0, late: 0.0 },
        "flat" => SingerKind::Natural { cents: -num(100.0)?, octave: 0, late: 0.0 },
        "cents" => SingerKind::Natural { cents: num(0.0)?, octave: 0, late: 0.0 },
        "octave" => SingerKind::Natural { cents: 0.0, octave: num(-1.0)? as i32, late: 0.0 },
        "late" => SingerKind::Natural { cents: 0.0, octave: 0, late: num(100.0)? / 1000.0 },
        "early" => SingerKind::Natural { cents: 0.0, octave: 0, late: -num(100.0)? / 1000.0 },
        "one-note" => SingerKind::OneNote,
        "rubato" => {
            let mut it = arg.unwrap_or("120").split(':');
            let range = it.next().unwrap_or("120").parse::<f64>()? / 1000.0;
            let seed = it.next().map(str::parse::<u64>).transpose()?.unwrap_or(1);
            SingerKind::Rubato { range, seed }
        }
        "behind" => SingerKind::Behind { shift: -num(0.0)? / 1000.0 },
        "ahead" => SingerKind::Ahead { shift: num(0.0)? / 1000.0 },
        "swap" => SingerKind::Swap,
        "staccato" => SingerKind::Staccato { share: num(0.5)? },
        "gap" => {
            let mut it = arg.unwrap_or("0.25:breath").split(':');
            let share = it.next().unwrap_or("0.25").parse::<f64>()?;
            let kind = match it.next().unwrap_or("breath") {
                "silence" => 0,
                "breath" => 1,
                "whisper" => 2,
                "speech" => 3,
                k => return Err(format!("gap kind {k}: silence | breath | whisper | speech").into()),
            };
            SingerKind::Gap { share, kind }
        }
        "silent" => SingerKind::Silent,
        "noise" => SingerKind::Noise,
        _ => return Err(format!("unknown singer {s}").into()),
    })
}

/// A scripted singer for the whole song at `rate`, following the key heard.
fn make_singer(song: &Song, kind: SingerKind, keys: &KeyTimeline, rate: u32, level_db: f64, vowel: Vowel) -> Vec<f32> {
    let dur = song.guide.duration_s;
    let voice = Voice { vowel, level_dbfs: level_db, breath_db: None, seed: 17 };
    let notes: Vec<ScoreNote> = song.guide.notes.iter().filter(|n| n.pitched).cloned().collect();
    let find = move |notes: &[ScoreNote], t: f64| -> Option<ScoreNote> {
        let i = notes.partition_point(|n| n.end <= t);
        notes.get(i).filter(|n| t >= n.start).cloned()
    };
    match kind {
        SingerKind::Silent => tv::silence(rate, dur),
        SingerKind::Noise => {
            let mut x = tv::room_noise(rate, dur, -35.0, Some((50.0, -55.0)), 23);
            let s = tv::speech_noise(rate, dur, -30.0, 29);
            for (a, b) in x.iter_mut().zip(&s) {
                *a += b;
            }
            x
        }
        SingerKind::OneNote => {
            // The median pitch of the graded notes, held whenever the guide sings.
            let mut p: Vec<f64> = song.guide.notes.iter().enumerate().filter(|(i, _)| song.guide.gradable(*i)).map(|(_, n)| n.midi).collect();
            p.sort_by(|a, b| a.total_cmp(b));
            let m = p.get(p.len() / 2).copied().unwrap_or(57.0);
            let k = keys.clone();
            tv::sing(rate, dur, voice, move |t| find(&notes, t).map(|_| tv::midi_hz(m + f64::from(k.key_at(t, None)))))
        }
        SingerKind::Steady => {
            let k = keys.clone();
            tv::sing(rate, dur, voice, move |t| find(&notes, t).map(|n| tv::midi_hz(n.midi + f64::from(k.key_at(t, None)))))
        }
        SingerKind::Natural { cents, octave, late } => {
            let k = keys.clone();
            tv::sing(rate, dur, voice, move |t| {
                let u = t - late;
                let n = find(&notes, u)?;
                let into = u - n.start;
                let len = n.end - n.start;
                let mut m = n.midi + f64::from(k.key_at(u, None)) + cents / 100.0 + 12.0 * f64::from(octave);
                if into < 0.06 {
                    m -= 0.8 * (1.0 - into / 0.06);
                }
                if len > 0.35 && into > 0.15 {
                    let depth = 0.3 * ((into - 0.15) / 0.1).min(1.0);
                    m += depth * (2.0 * std::f64::consts::PI * 5.5 * into).sin();
                }
                Some(tv::midi_hz(m))
            })
        }
        SingerKind::Gap { share, kind } => {
            let k = keys.clone();
            let gap = |n: &ScoreNote, t: f64| {
                let half = 0.5 * share * (n.end - n.start);
                (t - n.midpoint()).abs() < half
            };
            let nn = notes.clone();
            let mut x = tv::sing(rate, dur, voice, move |t| {
                let n = find(&nn, t)?;
                if gap(&n, t) {
                    return None;
                }
                Some(tv::midi_hz(natural_pitch(n.midi + f64::from(k.key_at(t, None)), t - n.start, n.end - n.start)))
            });
            let noise = match kind {
                1 => tv::breath(rate, dur, level_db, 41),
                2 => tv::fricatives(rate, dur, level_db, 43),
                3 => tv::speech_noise(rate, dur, level_db, 47),
                _ => vec![0.0; x.len()],
            };
            // Unvoiced sound only inside the gaps, with 5 ms ramps.
            let ramp = 0.005;
            for (i, v) in x.iter_mut().enumerate() {
                let t = i as f64 / f64::from(rate);
                if let Some(n) = find(&notes, t) {
                    let half = 0.5 * share * (n.end - n.start);
                    let d = half - (t - n.midpoint()).abs();
                    if d > 0.0 {
                        *v += noise.get(i).copied().unwrap_or(0.0) * (d / ramp).min(1.0) as f32;
                    }
                }
            }
            x
        }
        SingerKind::Rubato { .. } | SingerKind::Behind { .. } | SingerKind::Ahead { .. } | SingerKind::Swap | SingerKind::Staccato { .. } => {
            let spans = sung_spans(&notes, kind, keys);
            tv::sing(rate, dur, voice, move |t| {
                let i = spans.partition_point(|s| s.1 <= t);
                let &(s0, s1, m0) = spans.get(i).filter(|s| t >= s.0)?;
                Some(tv::midi_hz(natural_pitch(m0, t - s0, s1 - s0)))
            })
        }
    }
}

/// The natural singer's pitch `into` s into a sung note of `len` s at `midi`: a 60 ms scoop
/// from 80 cents below, then a +-30 cent vibrato at 5.5 Hz on notes longer than 0.35 s.
fn natural_pitch(midi: f64, into: f64, len: f64) -> f64 {
    let mut m = midi;
    if into < 0.06 {
        m -= 0.8 * (1.0 - into / 0.06);
    }
    if len > 0.35 && into > 0.15 {
        let depth = 0.3 * ((into - 0.15) / 0.1).min(1.0);
        m += depth * (2.0 * std::f64::consts::PI * 5.5 * into).sin();
    }
    m
}

/// What the timing singers and the adversaries sing: `(start, end, MIDI with the key heard)`,
/// ordered and not overlapping. `notes` are the guide's pitched notes.
fn sung_spans(notes: &[ScoreNote], kind: SingerKind, keys: &KeyTimeline) -> Vec<(f64, f64, f64)> {
    let key = |t: f64| f64::from(keys.key_at(t, None));
    let mut out: Vec<(f64, f64, f64)> = Vec::with_capacity(notes.len());
    match kind {
        SingerKind::Rubato { range, seed } => {
            let mut rng = tv::Rng::new(seed.wrapping_mul(7919).wrapping_add(13));
            let mut prev_start = f64::NEG_INFINITY;
            let mut prev_len = 0.0;
            for n in notes {
                let o = range * rng.bipolar();
                // The next onset waits until the previous note has had half its length.
                let s = (n.start + o).max(prev_start + 0.5 * prev_len);
                let e = n.end + o;
                if let Some(last) = out.last_mut() {
                    last.1 = last.1.min(s);
                }
                if e > s {
                    out.push((s, e, n.midi + key(n.midpoint())));
                }
                prev_start = s;
                prev_len = n.end - n.start;
            }
        }
        SingerKind::Behind { shift } => {
            for w in notes.windows(2) {
                out.push((w[1].start + shift, w[1].end + shift, w[0].midi + key(w[1].midpoint())));
            }
        }
        SingerKind::Ahead { shift } => {
            for w in notes.windows(2) {
                out.push((w[0].start + shift, w[0].end + shift, w[1].midi + key(w[0].midpoint())));
            }
        }
        SingerKind::Swap => {
            for p in notes.chunks(2) {
                if let [x, y] = p {
                    out.push((x.start, x.end, y.midi + key(x.midpoint())));
                    out.push((y.start, y.end, x.midi + key(y.midpoint())));
                }
            }
        }
        SingerKind::Staccato { share } => {
            for n in notes {
                out.push((n.start, n.start + share * (n.end - n.start), n.midi + key(n.midpoint())));
            }
        }
        _ => {}
    }
    // Ordered, not overlapping (a later onset cuts the note before it).
    out.sort_by(|a, b| a.0.total_cmp(&b.0));
    for i in 1..out.len() {
        if out[i - 1].1 > out[i].0 {
            out[i - 1].1 = out[i].0;
        }
    }
    out.retain(|s| s.1 > s.0);
    out
}

// ---------------------------------------------------------------------------------------
// The chain

/// The take's settings shared by `run` and `sweep`.
#[derive(Clone)]
struct TakeSetup {
    score: ScoreConfig,
    latency: LatencyConfig,
    block: usize,
    jitter_us: f64,
    /// Sample the music status every half second (evidence).
    trace: bool,
}

/// Plays `input` (interleaved) through the chain as if from the microphone, the song
/// starting at `start_s` on the first frame. Returns the result, the chain and the
/// count of values that showed a marker.
#[allow(clippy::too_many_arguments)]
fn play(
    guide: &ScoreGuide,
    excluded: &[(f64, f64)],
    keys: &KeyTimeline,
    setup: &TakeSetup,
    rate: u32,
    channels: usize,
    channel: usize,
    input: &[f32],
    reference: Option<&[f32]>,
    start_s: f64,
    completed: bool,
) -> (ScoreResult, Chain, u64, Vec<Value>) {
    let mut cfg = ChainConfig::new(rate, channels, channel);
    cfg.reference_channels = if reference.is_some() { 2 } else { 0 };
    cfg.score = setup.score.clone();
    cfg.latency = setup.latency.clone();
    let mut chain = Chain::new(cfg, guide.clone(), keys.clone(), excluded.to_vec());
    let frames = input.len() / channels;
    let block = setup.block.max(1);
    let host0: i64 = 7_000_000_000;
    let mut rng = tv::Rng::new(99);
    let mut live = PracticeLive::default();
    let mut shown = 0u64;
    let mut f = 0usize;
    let half = (rate / 2) as usize;
    let mut next_trace = 0usize;
    let mut trace = Vec::new();
    while f < frames {
        let n = block.min(frames - f);
        let exact = host0 + (f as f64 * 1e9 / f64::from(rate)).round() as i64;
        // The playback clock: the DAC played song time start + f / rate at that host time.
        chain.playback_anchor(ClockAnchor { host_ns: exact, song_s: start_s + f as f64 / f64::from(rate), segment: 0, playing: true });
        let stamp = exact + (rng.bipolar() * setup.jitter_us * 1000.0).round() as i64;
        let refb: &[f32] = match reference {
            Some(r) => &r[2 * f..2 * (f + n)],
            None => &[],
        };
        chain.process(stamp, &input[f * channels..(f + n) * channels], refb);
        chain.drain_live(&mut live);
        shown += live.pitch.iter().filter(|p| p.1.is_some()).count() as u64;
        f += n;
        if setup.trace && f >= next_trace {
            next_trace += half;
            let m = chain.music();
            let st = chain.stats();
            trace.push(json!([round3(f as f64 / f64::from(rate)), m.presence, m.removal, m.suppression_db.map(|x| (x * 10.0).round() / 10.0), m.delay_ms.map(round3), st.shown, st.dropped_music, st.dropped_unknown]));
        }
    }
    let r = chain.finish(completed, &[]);
    (r, chain, shown, trace)
}

fn strictness_of(s: Option<&str>, song: &Song) -> Res<Strictness> {
    match s {
        Some(x) => Strictness::parse(x).ok_or_else(|| format!("unknown strictness {x}").into()),
        None => Ok(song.strictness.unwrap_or(Strictness::Standard)),
    }
}

fn octave_of(s: Option<&str>, song: &Song) -> Res<(OctaveMode, i32)> {
    Ok(match s {
        None => (song.octave_mode, song.octave_shift),
        Some("auto") => (OctaveMode::Auto, song.octave_shift),
        Some("strict") => (OctaveMode::Strict, 0),
        Some(n) => (OctaveMode::Fixed, n.parse::<i32>()?.clamp(-2, 2)),
    })
}

fn result_hash(r: &ScoreResult) -> String {
    // FNV-1a of the result's JSON: equal hashes = identical results.
    let s = serde_json::to_string(r).unwrap_or_default();
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in s.bytes() {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    format!("{h:016x}")
}

fn cmd_run(a: &RunArgs) -> Res<()> {
    let t_all = Instant::now();
    let modes = usize::from(a.performance.is_some()) + usize::from(a.singer.is_some()) + usize::from(a.vocal);
    if modes != 1 {
        return Err("give exactly one of: a performance WAV, --singer SPEC, --vocal".into());
    }
    if a.channel >= a.channels {
        return Err("--channel must be below --channels".into());
    }
    let song = load_song(&a.song, true)?;
    let key0 = a.key.unwrap_or(song.saved.semitones);
    let base = song.saved.to_dsp();
    let changes = a.key_changes.as_deref().map(parse_changes).transpose()?.unwrap_or_default();
    let (events, mut keys) = key_plan(song.rate, jk_dsp::MixParams { semitones: key0, ..base }, key0, &changes)?;
    let mut start_s = a.start;
    if let Some(p) = &a.key_timeline {
        let (tl, s0) = engine_timeline(p, a.requested_keys)?;
        keys = tl;
        if a.start == 0.0 {
            start_s = s0;
        }
    }
    let (octave_mode, octave_shift) = octave_of(a.octave.as_deref(), &song)?;
    let strictness = strictness_of(a.strictness.as_deref(), &song)?;
    let vowel = match a.vowel.as_str() {
        "ee" => Vowel::Ee,
        "oo" => Vowel::Oo,
        _ => Vowel::Ah,
    };

    // The performance (the voice alone), at the input rate.
    let t_render = Instant::now();
    let (rate, voice_chans): (u32, Vec<Vec<f32>>) = if let Some(p) = &a.performance {
        let (r0, ch) = read_wav(p)?;
        let r = a.rate.unwrap_or(r0);
        (r, resample(ch, r0, r)?)
    } else if a.vocal {
        let r = a.rate.unwrap_or(song.rate);
        let st = render_mix(&song, vocal_params(key0), &shift_events(&events, vocal_params(key0)), r)?;
        (r, vec![mono(&st)])
    } else {
        let r = a.rate.unwrap_or(48_000);
        let kind = parse_singer(a.singer.as_deref().unwrap_or("perfect"))?;
        (r, vec![make_singer(&song, kind, &keys, r, a.level_db, vowel)])
    };
    if a.align_to_vocal {
        let r = render_mix(&song, vocal_params(key0), &[], rate)?;
        start_s = align_to(&voice_chans[0], &r[0], rate)?;
        eprintln!("aligned: the performance starts at song time {start_s:.6} s");
    }
    // A performance file with several channels already is the mixer's input.
    let file_layout = voice_chans.len() > 1;
    let mut voice = if file_layout { Vec::new() } else { voice_chans[0].clone() };

    // The app's playback at the input rate (the echo reference), and the music fed back.
    let fb = a.feedback.as_deref().map(parse_feedback).transpose()?;
    let reference: Option<Vec<f32>> = if a.no_reference {
        None
    } else {
        let mut p = base;
        p.semitones = key0;
        if let Some(g) = a.guide_vocal {
            p.guide = g;
        }
        let st = render_mix(&song, p, &shift_events(&events, p), rate)?;
        Some(interleave2(&st))
    };
    let render_s = t_render.elapsed().as_secs_f64();
    if !file_layout {
        let voice_delay = a.voice_delay_ms.unwrap_or_else(|| fb.as_ref().map(|f| f.delay_s * 1000.0).unwrap_or(0.0));
        if voice_delay > 0.0 {
            let d = (voice_delay / 1000.0 * f64::from(rate)).round() as usize;
            let mut v = vec![0.0f32; d];
            v.extend_from_slice(&voice[..voice.len().saturating_sub(d)]);
            voice = v;
        }
        if let Some(rv) = &a.reverb {
            let mut it = rv.split(',');
            let rt60: f64 = it.next().unwrap_or("0.6").parse()?;
            let wet: f64 = it.next().map(str::parse).transpose()?.unwrap_or(-6.0);
            voice = rig::reverb(&voice, rate, rt60, wet, 5);
        }
        match a.compress.as_deref() {
            None => {}
            Some("moderate") => voice = rig::compress(&voice, rate, Compressor::MODERATE),
            Some("heavy") => voice = rig::compress(&voice, rate, Compressor::HEAVY),
            Some(x) => return Err(format!("unknown compressor {x}").into()),
        }
    }
    let ret = match (&fb, &reference) {
        (Some(f), Some(r)) => Some(testpath::apply(&f.path, r, 2, rate)),
        (Some(_), None) => return Err("--feedback needs the reference (drop --no-reference)".into()),
        _ => None,
    };

    // The mixer's input: the voice (plus the returned music) on one channel.
    let (channels, channel, mut input) = if file_layout {
        let c = voice_chans.len();
        let n = voice_chans[0].len();
        let mut v = vec![0.0f32; n * c];
        for (k, ch) in voice_chans.iter().enumerate() {
            for i in 0..n {
                v[i * c + k] = ch[i];
            }
        }
        (c, a.channel.min(c - 1), v)
    } else {
        let n = voice.len();
        let c = a.channels.max(1);
        let other: Vec<f32> = match a.other.as_str() {
            "noise" => tv::room_noise(rate, n as f64 / f64::from(rate), -50.0, None, 31),
            "music" => reference.as_ref().map(|r| r.chunks(2).map(|f| 0.25 * (f[0] + f[1])).collect()).unwrap_or_else(|| vec![0.0; n]),
            _ => vec![0.0; n],
        };
        let mut v = vec![0.0f32; n * c];
        for i in 0..n {
            for k in 0..c {
                v[i * c + k] = if k == a.channel {
                    voice[i] + ret.as_ref().map(|r| r.0.get(i).copied().unwrap_or(0.0)).unwrap_or(0.0)
                } else {
                    other.get(i).copied().unwrap_or(0.0)
                };
            }
        }
        (c, a.channel, v)
    };
    // The part of the song played: from `start` for `seconds`.
    let total_frames = input.len() / channels;
    let first = if a.performance.is_some() || a.key_timeline.is_some() { 0 } else { (start_s * f64::from(rate)).round() as usize };
    let first = first.min(total_frames);
    let last = match a.seconds {
        Some(s) => (first + (s * f64::from(rate)).round() as usize).min(total_frames),
        None => total_frames,
    };
    input = input[first * channels..last * channels].to_vec();
    if let Some(p) = &a.write_input {
        write_wav(p, rate, &input, channels)?;
    }
    let song_start = if a.performance.is_some() || a.key_timeline.is_some() { start_s } else { first as f64 / f64::from(rate) };
    // The reference frame that played when each input frame was captured: song time
    // song_start + i / rate (zeros outside the song).
    let refslice = reference.as_ref().map(|r| {
        let n = last - first;
        let off = (song_start * f64::from(rate)).round() as i64;
        let mut v = vec![0.0f32; 2 * n];
        for i in 0..n {
            let s = off + i as i64;
            if s >= 0 && ((2 * s + 1) as usize) < r.len() {
                v[2 * i] = r[2 * s as usize];
                v[2 * i + 1] = r[2 * s as usize + 1];
            }
        }
        v
    });
    let completed = song_start + (last - first) as f64 / f64::from(rate) >= song.guide.duration_s - 0.5;
    let keys_used = keys.shifted(a.target_offset);
    let setup = TakeSetup {
        score: ScoreConfig {
            strictness,
            octave_mode,
            octave_shift,
            detail: a.notes,
            bridge_s: a.bridge_ms.map(|b| b / 1000.0).unwrap_or(ScoreConfig::default().bridge_s),
            ..a.scorer.apply(ScoreConfig::default())?
        },
        latency: LatencyConfig { auto: !a.no_auto_latency, stored_s: None, manual_s: a.latency_ms / 1000.0 },
        block: a.block.unwrap_or((rate / 100) as usize),
        jitter_us: a.jitter_us,
        trace: a.trace,
    };
    let t_chain = Instant::now();
    let (r, chain, shown, trace) = play(&song.guide, &song.excluded, &keys_used, &setup, rate, channels, channel, &input, refslice.as_deref(), song_start, completed);
    let chain_s = t_chain.elapsed().as_secs_f64();
    if let Some(p) = &a.dump_samples {
        dump_samples(p, &a.song, &chain)?;
    }
    let audio_s = (last - first) as f64 / f64::from(rate);
    let truth_delay_ms = ret.as_ref().map(|(_, d)| {
        let mid = d.len() / 2;
        d.get(mid).copied().unwrap_or(0.0) * 1000.0
    });
    let out = json!({
        "song": song.name,
        "performance": a.performance.as_ref().map(|p| p.display().to_string()),
        "singer": a.singer, "vocal": a.vocal,
        "key": key0, "key_changes": changes, "target_offset": a.target_offset,
        "key_timeline": keys_used,
        "rate": rate, "channels": channels, "channel": channel, "other": a.other,
        "reverb": a.reverb, "compress": a.compress,
        "feedback": a.feedback, "guide_vocal": a.guide_vocal,
        "feedback_truth_delay_ms_mid": truth_delay_ms,
        "start_s": song_start, "audio_s": audio_s,
        "result": r,
        "hash": result_hash(&r),
        "marker_values_shown": shown,
        "music": chain.music(),
        "latency": chain.latency(),
        "stats": chain.stats(),
        "echo": chain.echo_report(),
        "trace": trace,
        // With --notes: the guide's notes (start, end, MIDI, graded by the guide's rule),
        // indexed like `result.notes[].note`, for diagnostics (no lyrics).
        "guide_notes": if a.notes {
            json!(song.guide.notes.iter().enumerate().map(|(i, n)| json!([round3(n.start), round3(n.end), round3(n.midi), song.guide.gradable(i)])).collect::<Vec<_>>())
        } else {
            Value::Null
        },
        // With --notes: whether each guide note is pitched (sung) rather than spoken.
        "guide_pitched": if a.notes {
            json!(song.guide.notes.iter().map(|n| n.pitched).collect::<Vec<_>>())
        } else {
            Value::Null
        },
        // With --notes and a timing singer or adversary: what it sang (start, end, MIDI).
        "singer_spans": match (a.notes, a.performance.is_none() && !a.vocal, a.singer.as_deref().map(parse_singer)) {
            (true, true, Some(Ok(kind @ (SingerKind::Rubato { .. } | SingerKind::Behind { .. } | SingerKind::Ahead { .. } | SingerKind::Swap | SingerKind::Staccato { .. })))) => {
                let pitched: Vec<ScoreNote> = song.guide.notes.iter().filter(|n| n.pitched).cloned().collect();
                json!(sung_spans(&pitched, kind, &keys).iter().map(|&(s, e, m)| json!([round3(s), round3(e), round3(m)])).collect::<Vec<_>>())
            }
            _ => Value::Null,
        },
        "timing": { "render_s": render_s, "chain_s": chain_s, "chain_x_realtime": audio_s / chain_s.max(1e-9), "total_s": t_all.elapsed().as_secs_f64() },
    });
    emit(&out, a.out.as_deref())
}

/// `--dump-samples`: the scorer's input (one row per tracker value: song time, MIDI or
/// null, level dBFS, clarity, unknown, music) and the guide's pitch curve (MIDI per hop, 0 =
/// unvoiced) for per-note diagnostics. Pitch numbers only.
fn dump_samples(path: &Path, song: &Path, chain: &Chain) -> Res<()> {
    let rows: Vec<Value> = chain
        .scorer()
        .samples()
        .iter()
        .map(|s| {
            json!([
                (s.t * 1e4).round() / 1e4,
                s.midi.map(|m| (m * 1e3).round() / 1e3),
                (f64::from(s.level_db) * 10.0).round() / 10.0,
                (f64::from(s.clarity) * 1e3).round() / 1e3,
                u8::from(s.unknown),
                u8::from(s.music)
            ])
        })
        .collect();
    let f = jk_format::SongFile::open(song)?;
    let curve = jk_format::guide::read_guide(&f)?.and_then(|(_, c)| c).map(|c| {
        let midi = c.fields.first().map(|x| x.values.iter().map(|v| (f64::from(*v) * 1e3).round() / 1e3).collect::<Vec<_>>());
        json!({ "hop_s": f64::from(c.hop_us) / 1e6, "t0_s": f64::from(c.t0_us) / 1e6, "midi": midi })
    });
    let v = json!({ "samples": rows, "curve": curve });
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d)?;
    }
    std::fs::write(path, serde_json::to_string(&v)?)?;
    Ok(())
}

/// The events with the given base parameters (each event keeps only its key).
fn shift_events(events: &[ParamEvent], base: jk_dsp::MixParams) -> Vec<ParamEvent> {
    events.iter().map(|e| ParamEvent { frame: e.frame, params: jk_dsp::MixParams { semitones: e.params.semitones, ..base } }).collect()
}

struct Feedback {
    path: ReturnPath,
    delay_s: f64,
}

fn parse_feedback(s: &str) -> Res<Feedback> {
    let p: Vec<&str> = s.split(',').map(str::trim).collect();
    if p.len() != 3 {
        return Err("--feedback LEVEL_DB,DELAY_MS,KIND".into());
    }
    let level: f64 = p[0].parse()?;
    let delay_s = p[1].parse::<f64>()? / 1000.0;
    let path = match p[2] {
        "usb" => ReturnPath::usb(level, delay_s),
        "usb_eq" => ReturnPath::usb_eq(level, delay_s),
        "cable" => ReturnPath::cable(level, delay_s, 60.0),
        "bluetooth" => ReturnPath::bluetooth(level, delay_s),
        k => return Err(format!("unknown feedback kind {k}").into()),
    };
    Ok(Feedback { path, delay_s })
}

fn emit(v: &Value, out: Option<&Path>) -> Res<()> {
    let s = serde_json::to_string_pretty(v)?;
    match out {
        Some(p) => {
            if let Some(d) = p.parent() {
                std::fs::create_dir_all(d)?;
            }
            std::fs::write(p, s)?;
            println!("{}", p.display());
        }
        None => println!("{s}"),
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------
// sweep

fn cmd_sweep(a: &SweepArgs) -> Res<()> {
    let t_all = Instant::now();
    let song = load_song(&a.song, true)?;
    let (k0, k1) = a.keys.split_once(':').ok_or("--keys from:to")?;
    let keys: Vec<i32> = (k0.parse::<i32>()?..=k1.parse::<i32>()?).collect();
    let wrong: Vec<i32> = a.wrong.split(',').filter(|x| !x.trim().is_empty()).map(|x| x.trim().parse()).collect::<Result<_, _>>()?;
    if wrong.iter().any(|w| w % 12 == 0) {
        return Err("--wrong: a shift by whole octaves is not wrong (octaves are forgiven)".into());
    }
    let rate = a.rate.unwrap_or(song.rate);
    let threads = a.threads.unwrap_or_else(|| std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4)).max(1);
    let setup = TakeSetup {
        score: ScoreConfig {
            octave_mode: song.octave_mode,
            octave_shift: song.octave_shift,
            strictness: song.strictness.unwrap_or(Strictness::Standard),
            bridge_s: a.bridge_ms.map(|b| b / 1000.0).unwrap_or(ScoreConfig::default().bridge_s),
            ..a.scorer.apply(ScoreConfig::default())?
        },
        latency: LatencyConfig::default(),
        block: (rate / 100) as usize,
        jitter_us: 0.0,
        trace: false,
    };
    let next = AtomicUsize::new(0);
    let results: Vec<std::sync::Mutex<Option<Value>>> = keys.iter().map(|_| std::sync::Mutex::new(None)).collect();
    let errors = std::sync::Mutex::new(Vec::<String>::new());
    std::thread::scope(|sc| {
        for _ in 0..threads.min(keys.len()) {
            sc.spawn(|| loop {
                let i = next.fetch_add(1, Ordering::Relaxed);
                if i >= keys.len() {
                    break;
                }
                let k = keys[i];
                match sweep_one(&song, k, &wrong, rate, a.with_reference, &setup) {
                    Ok(v) => {
                        eprintln!("{} key {k:+}: {}", song.name, v["overall"]);
                        *results[i].lock().unwrap() = Some(v);
                    }
                    Err(e) => errors.lock().unwrap().push(format!("key {k}: {e}")),
                }
            });
        }
    });
    let errs = errors.into_inner().unwrap();
    if !errs.is_empty() {
        return Err(errs.join("; ").into());
    }
    let rows: Vec<Value> = results.into_iter().map(|m| m.into_inner().unwrap().unwrap_or(Value::Null)).collect();
    let at0 = rows.iter().find(|r| r["key"] == 0).and_then(|r| r["overall"].as_f64());
    let mut worst_diff = 0.0f64;
    let mut worst_wrong = 0.0f64;
    for r in &rows {
        if let (Some(o), Some(z)) = (r["overall"].as_f64(), at0) {
            worst_diff = worst_diff.max((o - z).abs());
        }
        if let Some(w) = r["wrong"].as_object() {
            for v in w.values() {
                worst_wrong = worst_wrong.max(v["overall"].as_f64().unwrap_or(0.0));
            }
        }
    }
    let out = json!({
        "song": song.name, "rate": rate, "with_reference": a.with_reference,
        "guide": song.guide.label, "notes": song.guide.notes.len(),
        "graded_notes_in_guide": (0..song.guide.notes.len()).filter(|&i| song.guide.gradable(i)).count(),
        "octave_mode": song.octave_mode, "strictness": setup.score.strictness,
        "key0_overall": at0, "max_abs_diff_from_key0": worst_diff, "max_overall_against_wrong_guides": worst_wrong,
        "keys": rows,
        "seconds": t_all.elapsed().as_secs_f64(), "threads": threads,
    });
    emit(&out, a.out.as_deref())
}

/// One key of the sweep: render the vocal at `key`, grade it at `key` through the chain,
/// then grade the same tracked singing against each wrongly shifted guide.
fn sweep_one(song: &Song, key: i32, wrong: &[i32], rate: u32, with_ref: bool, setup: &TakeSetup) -> Res<Value> {
    let t = Instant::now();
    let vocal = mono(&render_mix(song, vocal_params(key), &[], rate)?);
    let reference = if with_ref {
        let p = jk_dsp::MixParams { semitones: key, ..song.saved.to_dsp() };
        Some(interleave2(&render_mix(song, p, &[], rate)?))
    } else {
        None
    };
    let render_s = t.elapsed().as_secs_f64();
    let keys = KeyTimeline::constant(key);
    let (r, chain, shown, _) = play(&song.guide, &song.excluded, &keys, setup, rate, 1, 0, &vocal, reference.as_deref(), 0.0, true);
    let samples = chain.scorer().samples();
    let mut w = serde_json::Map::new();
    for &d in wrong {
        let mut s = Scorer::new(setup.score.clone(), song.guide.clone(), keys.shifted(d), song.excluded.clone(), song.guide.duration_s);
        for x in &samples {
            s.push(*x);
        }
        let rr = s.finish(&FinishContext { completed: true, flags: r.trust.clone() });
        w.insert(format!("{d:+}"), json!({ "overall": rr.overall, "grade": rr.grade, "tendency_cents": rr.tendency_cents }));
    }
    Ok(json!({
        "key": key, "overall": r.overall, "grade": r.grade, "graded": r.graded, "total": r.total,
        "tendency_cents": r.tendency_cents, "voiced_share": r.voiced_share, "octave": r.octave,
        "unknown_notes": r.unknown_notes, "trust": r.trust, "marker_values_shown": shown,
        "hash": result_hash(&r), "wrong": w, "render_s": render_s, "total_s": t.elapsed().as_secs_f64(),
    }))
}

fn round3(x: f64) -> f64 {
    (x * 1000.0).round() / 1000.0
}

/// Song time of `capture`'s first frame, by an exact match of 512 samples taken 0.4 s
/// after it first sounds (past the fade-in after a seek, before the first key change)
/// within `render` (the key-0 render, which an engine capture equals wherever key 0 is
/// heard).
fn align_to(capture: &[f32], render: &[f32], rate: u32) -> Res<f64> {
    const W: usize = 512;
    let first = capture.iter().position(|x| x.abs() > 1e-4).ok_or("the performance is silent")? + (rate as usize * 2) / 5;
    // If key 0 is not heard there (a change came before the singing), the last stretch
    // (back in key 0 after the plan's last change) is tried.
    let last = capture.len().saturating_sub(rate as usize / 2 + W);
    let mut err = String::new();
    for p in [first, last] {
        match align_at(capture, render, p, rate) {
            Ok(t) => return Ok(t),
            Err(e) => err = e.to_string(),
        }
    }
    Err(err.into())
}

fn align_at(capture: &[f32], render: &[f32], p: usize, rate: u32) -> Res<f64> {
    const W: usize = 512;
    if p + W > capture.len() {
        return Err("the performance is too short to align".into());
    }
    let pat = &capture[p..p + W];
    if pat.iter().all(|x| x.abs() < 1e-4) {
        return Err("silent where it was to be aligned".into());
    }
    let mut best = (f64::MAX, 0usize);
    for q in 0..render.len().saturating_sub(W) {
        let mut e = 0.0f64;
        for k in 0..W {
            let d = f64::from(pat[k] - render[q + k]);
            e += d * d;
            if e >= best.0 {
                break;
            }
        }
        if e < best.0 {
            best = (e, q);
        }
    }
    if best.0 > 1e-6 {
        return Err(format!("no exact match with the key-0 vocal (residual {:.3e})", best.0).into());
    }
    Ok((best.1 as f64 - p as f64) / f64::from(rate))
}

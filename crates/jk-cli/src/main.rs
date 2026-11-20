// SPDX-License-Identifier: GPL-3.0-or-later
//! `jukeoke-cli`: developer and test tool for the audio engine and song files.
//!
//! Song files (`.jksong`):
//! * `info <song>`: manifest, stems, lyrics and saved settings.
//! * `verify <song> [--strict] [--deep]`: SONG_FORMAT section 9 checklist
//!   (jk_format::verify_file: every CRC, every stem fully decoded with MD5 and seek
//!   checks, the 1.1 guide checks; `--deep` also compares the guide's source SHA-256 with
//!   the lead vocal's bytes) plus a decode through the engine's own stem reader. Exit
//!   status 1 on any error.
//! * `guide add SONG... [--force] [--json]` (alias `guide build`): Work Order 2, adds the
//!   melody guide from the song's own lead vocal and lyrics by the raw-copy rewrite of
//!   SONG_FORMAT 7.1 (STUDIO_PROTOCOL 12.3). `guide show SONG [--json]` (alias
//!   `guide info`): status, counts, tuning and provenance, never the lyrics.
//! * `render <song> --out f.wav|f.flac [--semitones N ...]`: offline render with the
//!   song's saved settings, overridden by the options given.
//! * `export <song> --format flac24|wav32f [--out f]`: "Export backing track" with the
//!   song's saved settings (what the app's export does).
//! * `play <song> ...`: play through an output device through the real engine API and
//!   report underruns, worker lead, clock drift, memory (long-run benchmark driver).
//! * `seek-test <song>`: seek accuracy (null device, sample comparison with the offline
//!   render) and seek latency (real device).
//! * `make-demo-song <out.jksong>`: the generated demo song (public-domain text,
//!   synthesised audio, letter timings exact by construction).
//! * `make-test-song <out.jksong> --seconds 240`: a long stereo synthetic song.
//! * `key-switch-capture <song> --out DIR --changes 2:-3,5:4`: Work Order 2 key in effect:
//!   play the lead vocal alone in real time on the null device, change key during
//!   playback, write the played audio and the key timeline the engine published.
//! * `devices`: list output devices.
//! * `practice run SONG (PERF.wav | --singer SPEC | --vocal) ...` and `practice sweep SONG`:
//!   Work Order 2 offline practice harness: a performance through the app's practice
//!   analysis chain (jk_pitch::practice) with the playback rendered as the echo reference;
//!   result JSON (see `practice.rs`).
//!
//! Engine development (stem folders, generated material):
//! * `gen-test-stems`, `render-stems`, `bench`, `ci-render` (Windows-vs-macOS renders).

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering::Relaxed};
use std::sync::Arc;
use std::time::{Duration, Instant};

use clap::{Args, Parser, Subcommand};
use jk_dsp::{render, MixParams as DspParams, ParamEvent, RenderOptions, StemsRef};
use jk_engine::{Engine, EngineConfig, ExportFormat, FlacStem, MixParams, PlaybackState};

type Res<T> = Result<T, Box<dyn std::error::Error>>;

// Work Order 2: the practice harness (`practice run`, `practice sweep`).
mod practice;

#[derive(Parser)]
#[command(name = "jukeoke-cli", version, about = "Jukeoke audio engine and song-file tool")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Show a song file's metadata, stems, lyrics and saved settings.
    Info { song: PathBuf },
    /// Check a song file completely (format checklist + full decode). Exit 1 on errors.
    Verify {
        song: PathBuf,
        /// Warnings fail too.
        #[arg(long)]
        strict: bool,
        /// Also compare the melody guide's source SHA-256 with the lead vocal's bytes.
        #[arg(long)]
        deep: bool,
    },
    /// Melody guide (Work Order 2): add one to songs, or show a song's.
    Guide {
        #[command(subcommand)]
        cmd: GuideCmd,
    },
    /// Offline render (same graph as playback) to a 32-bit float WAV or 24-bit FLAC.
    Render {
        song: PathBuf,
        #[arg(long)]
        out: PathBuf,
        /// flac24 or wav32f (default: from the extension of --out).
        #[arg(long)]
        format: Option<String>,
        #[command(flatten)]
        mix: MixOverride,
    },
    /// Export the backing track with the song's saved settings.
    Export {
        song: PathBuf,
        #[arg(long, default_value = "flac24")]
        format: String,
        /// Output file (default: next to the song, "<artist> - <title> (<key>).<ext>").
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// Play a song through an output device and report timing statistics.
    Play(PlayArgs),
    /// Measure seek accuracy (null device) and seek latency (real device).
    SeekTest {
        song: PathBuf,
        /// Seeks to test.
        #[arg(long, default_value_t = 12)]
        seeks: usize,
        /// Output device for the latency part ("null" for none).
        #[arg(long)]
        device: Option<String>,
    },
    /// Write the generated demo song.
    MakeDemoSong { out: PathBuf },
    /// Write a long synthetic stereo test song (no lyrics).
    MakeTestSong {
        out: PathBuf,
        #[arg(long, default_value_t = 240.0)]
        seconds: f64,
        #[arg(long, default_value_t = 44_100)]
        rate: u32,
        #[arg(long, default_value_t = 3)]
        seed: u32,
        /// Instead: a steady sine of this frequency (Hz) in the lead stem, other stems
        /// silent (Work Order 2 key-in-effect measurements).
        #[arg(long)]
        tone: Option<f64>,
    },
    /// List the output devices.
    Devices,
    /// Write synthetic 24-bit FLAC stems (lead, backing, drums, other) and their sum.
    GenTestStems {
        /// Output folder (default: $JK_BUILD/fixtures/synth-<rate>).
        #[arg(long)]
        out: Option<PathBuf>,
        #[arg(long, default_value_t = 90.0)]
        seconds: f64,
        #[arg(long, default_value_t = 44_100)]
        rate: u32,
        #[arg(long, default_value_t = 1)]
        seed: u32,
    },
    /// Offline render of a stem folder (lead/backing/drums/other.flac) to a float WAV.
    RenderStems {
        #[arg(long)]
        stems: PathBuf,
        #[arg(long)]
        out: PathBuf,
        /// Output rate (default: the song rate, no resampling).
        #[arg(long)]
        rate: Option<u32>,
        #[command(flatten)]
        mix: MixArgs,
    },
    /// Measure the CPU cost of Rubber Band R3 and of the full graph (run with --release).
    Bench {
        #[arg(long, default_value_t = 30.0)]
        seconds: f64,
    },
    /// Render a fixed set of generated test cases to OUT_DIR (CI cross-platform check).
    CiRender { out_dir: PathBuf },
    /// Work Order 2, key in effect: play the song's lead vocal alone on the null device in
    /// real time, change key during playback, and write the played audio (OUT/capture.wav,
    /// 32-bit float) and the key timeline the engine published (OUT/keys.json), so the
    /// moment the pitch really changes can be measured offline. Plays nothing aloud.
    KeySwitchCapture {
        song: PathBuf,
        /// Output folder.
        #[arg(long)]
        out: PathBuf,
        /// `realtime` or `prerender` (forced; uses --cache).
        #[arg(long, default_value = "realtime")]
        mode: String,
        /// Key cache folder for prerender mode (default: OUT/keycache).
        #[arg(long)]
        cache: Option<PathBuf>,
        /// Song time to start playing from (seconds).
        #[arg(long, default_value_t = 0.0)]
        start: f64,
        /// Key changes as `seconds:key` after play starts, comma-separated (`2:-3,5:4,8:0`).
        #[arg(long, allow_hyphen_values = true)]
        changes: String,
        /// How long to play (seconds).
        #[arg(long, default_value_t = 12.0)]
        seconds: f64,
        /// Play the whole mix with the guide vocal instead of the lead vocal alone.
        #[arg(long)]
        full_mix: bool,
    },
    /// Work Order 2 practice harness: grade a performance through the practice analysis
    /// chain as if it were the microphone (generated sound only).
    Practice {
        #[command(subcommand)]
        cmd: practice::PracticeCmd,
    },
}

#[derive(Subcommand)]
enum GuideCmd {
    /// Add a guide to each song that has none, or a stale or outdated one (STUDIO_PROTOCOL
    /// 12.3). Exit 0 when every song has a current guide afterwards, 1 when one failed,
    /// 2 on a usage error.
    #[command(visible_alias = "build")]
    Add {
        #[arg(required = true)]
        songs: Vec<PathBuf>,
        /// Rebuild a current guide too (never one from a newer version).
        #[arg(long)]
        force: bool,
        /// One JSON line per song: {"song","ok","state","error_code","message","notes","seconds"}.
        #[arg(long)]
        json: bool,
    },
    /// Status, counts, tuning and provenance of a song's guide (never the lyrics).
    #[command(visible_alias = "info")]
    Show {
        song: PathBuf,
        #[arg(long)]
        json: bool,
    },
}

#[derive(Args, Clone)]
struct PlayArgs {
    song: PathBuf,
    #[arg(long, default_value_t = 10.0)]
    seconds: f64,
    /// Master gain in dB (default -60 dB: inaudible test level).
    #[arg(long, default_value_t = -60.0, allow_hyphen_values = true)]
    master_db: f64,
    /// Play digital silence (master gain 0).
    #[arg(long)]
    silence: bool,
    /// Allow a master gain above -60 dB.
    #[arg(long)]
    loud: bool,
    /// Device id (see `devices`; "null" = no output).
    #[arg(long)]
    device: Option<String>,
    #[arg(long)]
    exclusive: bool,
    /// Busy-loop threads to run in parallel (CPU load test).
    #[arg(long, default_value_t = 0)]
    load_threads: usize,
    /// Change key every N seconds through +3, -2, +5, 0, -7, +12, -12, +2 (0 = off).
    #[arg(long, default_value_t = 0.0)]
    key_every: f64,
    /// Seek to a pseudo-random position every N seconds (0 = off).
    #[arg(long, default_value_t = 0.0)]
    seek_every: f64,
    /// Start again from 0 at the end of the song.
    #[arg(long = "loop")]
    looping: bool,
    #[arg(long, default_value_t = 0.0)]
    start: f64,
    /// Write a JSON summary here.
    #[arg(long)]
    report: Option<PathBuf>,
    #[command(flatten)]
    mix: MixOverride,
}

/// Mix options for song-file commands: anything not given comes from the song's
/// saved settings.
#[derive(Args, Clone, Default)]
struct MixOverride {
    #[arg(long, allow_hyphen_values = true)]
    semitones: Option<i32>,
    #[arg(long)]
    guide: Option<f32>,
    #[arg(long)]
    backing: Option<f32>,
    #[arg(long)]
    no_backing: bool,
    #[arg(long)]
    other: Option<f32>,
    #[arg(long)]
    drums: Option<f32>,
    #[arg(long)]
    shift_drums: bool,
}

impl MixOverride {
    fn apply(&self, mut p: MixParams) -> MixParams {
        if let Some(s) = self.semitones {
            p.semitones = s;
        }
        if let Some(g) = self.guide {
            p.guide_vocal = g;
        }
        if let Some(b) = self.backing {
            p.backing_level = b;
        }
        if self.no_backing {
            p.backing_on = false;
        }
        if let Some(o) = self.other {
            p.other = o;
        }
        if let Some(d) = self.drums {
            p.drums = d;
        }
        if self.shift_drums {
            p.shift_drums = true;
        }
        p
    }
}

#[derive(Args, Clone)]
struct MixArgs {
    #[arg(long, default_value_t = 0, allow_hyphen_values = true)]
    semitones: i32,
    #[arg(long, default_value_t = 0.0)]
    guide: f32,
    #[arg(long, default_value_t = 1.0)]
    backing: f32,
    #[arg(long)]
    no_backing: bool,
    #[arg(long, default_value_t = 1.0)]
    other: f32,
    #[arg(long, default_value_t = 1.0)]
    drums: f32,
    #[arg(long)]
    shift_drums: bool,
}

impl MixArgs {
    fn params(&self) -> DspParams {
        DspParams {
            guide: self.guide,
            backing_on: !self.no_backing,
            backing: self.backing,
            other: self.other,
            drums: self.drums,
            shift_drums: self.shift_drums,
            semitones: self.semitones,
        }
    }
}

fn main() {
    let cli = Cli::parse();
    let r = match cli.cmd {
        Cmd::Info { song } => cmd_info(&song),
        Cmd::Verify { song, strict, deep } => cmd_verify(&song, strict, deep),
        Cmd::Guide { cmd: GuideCmd::Add { songs, force, json } } => cmd_guide_add(&songs, force, json),
        Cmd::Guide { cmd: GuideCmd::Show { song, json } } => cmd_guide_show(&song, json),
        Cmd::Render { song, out, format, mix } => cmd_render(&song, &out, format.as_deref(), &mix),
        Cmd::Export { song, format, out } => cmd_export(&song, &format, out),
        Cmd::Play(a) => {
            if !a.silence && !a.loud && a.master_db > -60.0 {
                Err("master gain above -60 dB needs --loud".into())
            } else {
                cmd_play(&a)
            }
        }
        Cmd::SeekTest { song, seeks, device } => cmd_seek_test(&song, seeks, device),
        Cmd::MakeDemoSong { out } => cmd_make_demo(&out),
        Cmd::MakeTestSong { out, seconds, rate, seed, tone } => cmd_make_test_song(&out, seconds, rate, seed, tone),
        Cmd::Devices => cmd_devices(),
        Cmd::GenTestStems { out, seconds, rate, seed } => gen_test_stems(out, seconds, rate, seed),
        Cmd::RenderStems { stems, out, rate, mix } => cmd_render_stems(&stems, &out, rate, mix.params()),
        Cmd::Bench { seconds } => cmd_bench(seconds),
        Cmd::CiRender { out_dir } => cmd_ci_render(&out_dir),
        Cmd::KeySwitchCapture { song, out, mode, cache, start, changes, seconds, full_mix } => {
            cmd_key_switch_capture(&song, &out, &mode, cache, start, &changes, seconds, full_mix)
        }
        Cmd::Practice { cmd } => practice::run(cmd),
    };
    if let Err(e) = r {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

fn jk_build() -> PathBuf {
    std::env::var_os("JK_BUILD").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("../jukeoke-build"))
}

fn saved_params(song: &Path) -> Res<MixParams> {
    let f = jk_format::SongFile::open(song)?;
    Ok(MixParams::from_settings(f.effective_settings()))
}

fn db(x: f64) -> String {
    if x <= 0.0 { "-inf dB".into() } else { format!("{:.2} dB", 20.0 * x.log10()) }
}

// ---------------------------------------------------------------------------------------
// guide (Work Order 2, STUDIO_PROTOCOL 12.3)

/// `guide add`: one line per song (`--json`: the JSON of STUDIO_PROTOCOL 12.3).
fn cmd_guide_add(songs: &[PathBuf], force: bool, json: bool) -> Res<()> {
    let cfg = jk_pitch::guide::GuideConfig::default();
    let app = jk_guide::app_stamp("jukeoke-cli", env!("CARGO_PKG_VERSION"));
    let mut failed = 0usize;
    for song in songs {
        let r = jk_guide::add_to_song(song, &cfg, &app, force);
        let (ok, state, code, message, notes, seconds) = match &r {
            Ok(o) => match &o.built {
                Some(b) => (true, "ready".to_string(), None, "added".to_string(), Some(b.doc.notes.len()), o.total_seconds),
                None => {
                    // Nothing to do: report the state found (ready, or newer: kept).
                    let n = jk_format::SongFile::open(song)
                        .ok()
                        .and_then(|s| jk_guide::file_state(&s).1)
                        .map(|d| d.notes.len());
                    let msg = match &o.before {
                        jk_format::GuideFileState::Newer => "kept (made by a newer version)",
                        _ => "already current",
                    };
                    (true, jk_guide::state_name(&o.before).to_string(), None, msg.to_string(), n, o.total_seconds)
                }
            },
            Err(e) => (false, "failed".to_string(), Some(e.code), e.message.clone(), None, 0.0),
        };
        if !ok {
            failed += 1;
        }
        if json {
            println!(
                "{}",
                serde_json::json!({
                    "song": song.display().to_string(), "ok": ok, "state": state, "error_code": code,
                    "message": message, "notes": notes, "seconds": (seconds * 1000.0).round() / 1000.0,
                })
            );
        } else {
            match &r {
                Err(e) => println!("{}: FAILED {} - {}", song.display(), e.code, e.message),
                Ok(_) => println!(
                    "{}: {state}, {message}{}, {seconds:.2} s",
                    song.display(),
                    notes.map(|n| format!(", {n} notes")).unwrap_or_default()
                ),
            }
            if let Err(e) = &r {
                eprintln!("  detail: {}", e.detail);
            }
        }
    }
    if failed > 0 {
        std::process::exit(1);
    }
    Ok(())
}

/// `guide show`: never prints lyrics (word references are counted, not shown).
fn cmd_guide_show(song: &Path, json: bool) -> Res<()> {
    let f = jk_format::SongFile::open(song)?;
    let (state, doc) = jk_guide::file_state(&f);
    let reason = match &state {
        jk_format::GuideFileState::Stale(r) => Some(r.clone()),
        _ => None,
    };
    let entry = |id: &str| f.chunk(id).map(|c| c.length);
    let (gb, cb) = (entry(jk_format::guide::GUIDE_CHUNK), entry(jk_format::guide::CURVE_CHUNK));
    if json {
        let v = match &doc {
            Some(d) => serde_json::json!({
                "song": song.display().to_string(), "state": jk_guide::state_name(&state), "reason": reason,
                "counts": d.compute_counts(), "spans": d.spans.len(),
                "notes_with_word": d.notes.iter().filter(|n| n.word.is_some()).count(),
                "tuning": d.tuning, "range": d.range, "provenance": d.provenance,
                "guide_bytes": gb, "curve_bytes": cb,
                "builder": {"method": jk_guide::METHOD, "version": jk_guide::VERSION},
            }),
            None => serde_json::json!({
                "song": song.display().to_string(), "state": jk_guide::state_name(&state), "reason": reason,
                "builder": {"method": jk_guide::METHOD, "version": jk_guide::VERSION},
            }),
        };
        println!("{v}");
        return Ok(());
    }
    println!("song: {}", song.display());
    println!("state: {}{}", jk_guide::state_name(&state), reason.map(|r| format!(" ({r})")).unwrap_or_default());
    println!("this app's builder: {} {}", jk_guide::METHOD, jk_guide::VERSION);
    let Some(d) = doc else { return Ok(()) };
    let c = d.compute_counts();
    let p = &d.provenance;
    println!(
        "notes: {} ({} pitched, {} unpitched, {} low confidence, {} octave flags, {} graded); {} spoken spans; {} notes have a word",
        c.notes,
        c.pitched,
        c.unpitched,
        c.low_confidence,
        c.octave_flags,
        c.graded,
        d.spans.len(),
        d.notes.iter().filter(|n| n.word.is_some()).count()
    );
    if let Some(r) = &d.range {
        println!("range: MIDI {:.2} .. {:.2}", r.low, r.high);
    }
    println!("tuning: {:+.1} cents, strength {:.3}, used {:+.1} cents", d.tuning.cents, d.tuning.strength, d.tuning.use_cents);
    println!(
        "made by: {} {} on {} ({}), {} s, {}",
        p.method,
        p.version,
        p.app.as_ref().map(|a| format!("{} {} {} {}", a.name, a.version, a.platform, a.arch)).unwrap_or_default(),
        p.created_utc.as_deref().unwrap_or("?"),
        p.seconds.map(|s| format!("{s:.2}")).unwrap_or_else(|| "?".into()),
        p.source
            .as_ref()
            .map(|s| format!("from {} ({} bytes, crc32 {:08x}, sha256 {}...)", s.chunk, s.length, s.crc32, &s.sha256[..s.sha256.len().min(16)]))
            .unwrap_or_default()
    );
    println!("size: guide {} bytes, curve {} bytes", gb.unwrap_or(0), cb.unwrap_or(0));
    Ok(())
}

// ---------------------------------------------------------------------------------------
// info / verify

fn cmd_info(path: &Path) -> Res<()> {
    let song = jk_format::SongFile::open(path)?;
    let m = song.manifest();
    let meta = song.effective_metadata();
    let bytes = std::fs::metadata(path)?.len();
    println!("file: {} ({} bytes)", path.display(), bytes);
    println!("format: {} {}", m.format, m.format_version);
    println!("title: {}\nartist: {}\nalbum: {}", meta.title, meta.artist, meta.album.as_deref().unwrap_or("-"));
    println!("audio: {} Hz, {} ch, {} frames ({:.3} s)", m.sample_rate, m.channels, m.duration_frames, m.duration());
    if let Some(a) = &m.analysis {
        if let Some(k) = &a.key {
            println!("key: {} {} (confidence {:?})", k.tonic, k.mode, k.confidence);
        }
        if let Some(t) = a.tempo_bpm {
            println!("tempo: {t} BPM");
        }
    }
    println!("preset: {:?}, stems sum to source: {}", m.preset, m.stems_sum_to_source);
    for s in &m.stems {
        let len = song.chunk(&s.chunk).map_or(0, |c| c.length);
        println!(
            "stem {:<15} chunk {:<22} {:>10} bytes  gain {}  peak {:?}  in karaoke mix {}  pitched {}",
            s.role,
            s.chunk,
            len,
            s.gain,
            s.peak,
            s.in_karaoke_mix(),
            s.pitched()
        );
    }
    match song.effective_lyrics()? {
        Some(l) => {
            println!(
                "lyrics: {} lines, {} graphemes, source {:?}, checked {}",
                l.lines.len(),
                l.grapheme_count(),
                l.source,
                l.checked
            );
            for (i, line) in l.lines.iter().enumerate() {
                println!("  {i:2} [{:7.3} .. {:7.3}] {}", line.start, line.end, line.text());
            }
        }
        None => println!("lyrics: none (instrumental)"),
    }
    let s = song.effective_settings();
    println!(
        "saved settings: semitones {:+}, shift drums {}, guide {}, backing {} x{}, levels {:?}, lyrics offset {} ms",
        s.semitones, s.shift_drums, s.guide_vocal, s.backing_on, s.backing_level, s.levels, s.lyrics_offset_ms
    );
    match jk_guide::file_state(&song) {
        (state, Some(doc)) => {
            let c = doc.compute_counts();
            println!(
                "guide: {} ({} {}), {} notes, {} graded, tuning {:+.1} c used",
                jk_guide::state_name(&state),
                doc.provenance.method,
                doc.provenance.version,
                c.notes,
                c.graded,
                doc.tuning.use_cents
            );
        }
        (state, None) => println!("guide: {}", jk_guide::state_name(&state)),
    }
    println!("chunks:");
    for c in &song.toc().chunks {
        println!("  {:<22} {:<18} offset {:>10} length {:>10}", c.id, c.mime, c.offset, c.length);
    }
    for w in song.warnings() {
        println!("WARNING {w}");
    }
    Ok(())
}

fn cmd_verify(path: &Path, strict: bool, deep: bool) -> Res<()> {
    let t = Instant::now();
    let rep = jk_format::verify_file(path, &jk_format::VerifyOptions { deep, ..Default::default() });
    println!("file: {}", rep.path.display());
    if let Some((ma, mi)) = rep.version {
        println!("version: {ma}.{mi}");
    }
    for s in &rep.stems {
        println!(
            "stem {:<15} {} Hz {} ch {} bit, {} frames, {} bytes, MD5 {}, decoded {:?} frames, seeks {:?}, peak {:?}, seek points {:?}",
            s.role,
            s.sample_rate,
            s.channels,
            s.bits_per_sample,
            s.total_samples,
            s.bytes,
            match s.md5_ok {
                Some(true) => "ok",
                Some(false) => "MISMATCH",
                None => "not checked",
            },
            s.decoded_frames,
            s.seeks_ok,
            s.decoded_peak,
            s.seek_points
        );
    }
    for sl in &rep.slots {
        println!("{sl}");
    }
    if let Some(g) = rep.lyrics_graphemes {
        println!("lyrics: {g} timed graphemes");
    }
    if let Some(g) = &rep.guide {
        println!("guide: {g}");
    }
    // Decode through the engine's own stem reader (what playback uses).
    let mut errors = rep.errors.clone();
    match jk_engine::source::SongSource::open(path) {
        Ok(mut src) => {
            let frames = src.info().frames;
            let mut bufs = jk_engine::source::slot_buffers(8192);
            let mut n = 0u64;
            let mut peak = 0.0f32;
            loop {
                match src.read(&mut bufs, 8192) {
                    Ok(0) => break,
                    Ok(k) => {
                        for s in &bufs {
                            for c in s {
                                peak = c[..k].iter().fold(peak, |m, v| m.max(v.abs()));
                            }
                        }
                        n += k as u64;
                    }
                    Err(e) => {
                        errors.push(format!("engine decode: {e}"));
                        break;
                    }
                }
            }
            if n != frames {
                errors.push(format!("engine decode: {n} frames, manifest says {frames}"));
            } else {
                println!("engine stem reader: {n} frames decoded (all stems, gains applied), slot peak {}", db(peak as f64));
            }
        }
        Err(e) => errors.push(format!("engine open: {e}")),
    }
    for w in &rep.warnings {
        println!("WARNING {w}");
    }
    for e in &errors {
        println!("ERROR {e}");
    }
    let ok = errors.is_empty() && (!strict || rep.warnings.is_empty());
    println!(
        "{}: {} errors, {} warnings ({:.2} s)",
        if ok { "PASS" } else { "FAIL" },
        errors.len(),
        rep.warnings.len(),
        t.elapsed().as_secs_f64()
    );
    if !ok {
        std::process::exit(1);
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------
// render / export

fn parse_format(s: &str) -> Res<ExportFormat> {
    match s.to_ascii_lowercase().as_str() {
        "flac24" | "flac" => Ok(ExportFormat::Flac24),
        "wav32f" | "wav" => Ok(ExportFormat::Wav32f),
        _ => Err(format!("unknown format {s} (flac24 or wav32f)").into()),
    }
}

fn run_render(song: &Path, p: &MixParams, out: &Path, fmt: ExportFormat) -> Res<()> {
    let t = Instant::now();
    let cancel = AtomicBool::new(false);
    let mut last = -1.0f32;
    let mut progress = |x: f32| {
        if x - last >= 0.25 || x >= 1.0 {
            last = x;
            eprint!("\r{:3.0} %", x * 100.0);
        }
    };
    let r = jk_engine::render_offline(song, p, out, fmt, &mut progress, &cancel)?;
    eprintln!();
    let secs = t.elapsed().as_secs_f64();
    let dur = r.frames as f64 / r.sample_rate as f64;
    println!(
        "{}: {:?}, {} frames at {} Hz, {} ch; key {:+}; peak {}; applied gain {:.3} dB; {:.2} s ({:.1}x real time)",
        out.display(),
        fmt,
        r.frames,
        r.sample_rate,
        r.channels,
        p.semitones,
        db(r.peak as f64),
        r.applied_gain_db,
        secs,
        dur / secs
    );
    Ok(())
}

fn cmd_render(song: &Path, out: &Path, format: Option<&str>, mix: &MixOverride) -> Res<()> {
    let fmt = match format {
        Some(f) => parse_format(f)?,
        None => match out.extension().and_then(|e| e.to_str()).map(|e| e.to_ascii_lowercase()).as_deref() {
            Some("flac") => ExportFormat::Flac24,
            _ => ExportFormat::Wav32f,
        },
    };
    let p = mix.apply(saved_params(song)?);
    run_render(song, &p, out, fmt)
}

fn cmd_export(song: &Path, format: &str, out: Option<PathBuf>) -> Res<()> {
    let fmt = parse_format(format)?;
    let f = jk_format::SongFile::open(song)?;
    let p = MixParams::from_settings(f.effective_settings());
    let out = match out {
        Some(o) => o,
        None => {
            let meta = f.effective_metadata();
            let key = f
                .manifest()
                .analysis
                .as_ref()
                .and_then(|a| a.key.as_ref())
                .map(|k| format!("{} {} {:+}", k.tonic, k.mode, p.semitones))
                .unwrap_or_else(|| format!("{:+}", p.semitones));
            let ext = if fmt == ExportFormat::Flac24 { "flac" } else { "wav" };
            let name: String = format!("{} - {} ({key}).{ext}", meta.artist, meta.title)
                .chars()
                .map(|c| if "<>:\"/\\|?*".contains(c) || c.is_control() { '_' } else { c })
                .collect();
            song.with_file_name(name)
        }
    };
    drop(f);
    run_render(song, &p, &out, fmt)
}

// ---------------------------------------------------------------------------------------
// generators

fn cmd_make_demo(out: &Path) -> Res<()> {
    let t = Instant::now();
    let r = jk_engine::demo::make_demo_song(out)?;
    println!(
        "{}: {} bytes, {} frames at {} Hz ({:.1} s); stems: {}; {:.2} s",
        r.path.display(),
        r.bytes,
        r.frames,
        r.sample_rate,
        r.frames as f64 / r.sample_rate as f64,
        r.stem_bytes.iter().map(|(k, b)| format!("{k} {b} B")).collect::<Vec<_>>().join(", "),
        t.elapsed().as_secs_f64()
    );
    Ok(())
}

fn cmd_make_test_song(out: &Path, seconds: f64, rate: u32, seed: u32, tone: Option<f64>) -> Res<()> {
    let t = Instant::now();
    let r = match tone {
        Some(f0) => jk_engine::demo::make_tone_song(out, seconds, rate, f0)?,
        None => jk_engine::demo::make_synth_song(out, seconds, rate, seed)?,
    };
    println!(
        "{}: {} bytes, {} frames at {} Hz ({:.1} s), stereo 24-bit; {:.1} s",
        r.path.display(),
        r.bytes,
        r.frames,
        r.sample_rate,
        r.frames as f64 / r.sample_rate as f64,
        t.elapsed().as_secs_f64()
    );
    Ok(())
}

fn cmd_devices() -> Res<()> {
    for d in Engine::list_devices()? {
        println!(
            "{} {} | {} Hz {} ch | exclusive {} | id {}",
            if d.is_default { "*" } else { " " },
            d.name,
            d.sample_rate,
            d.channels,
            if d.exclusive_capable { "yes" } else { "no" },
            d.id
        );
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------
// play

fn busy_threads(n: usize, stop: Arc<AtomicBool>) -> Vec<std::thread::JoinHandle<u64>> {
    (0..n)
        .map(|i| {
            let stop = stop.clone();
            std::thread::spawn(move || {
                let mut x = 1.0f64 + i as f64;
                let mut iters = 0u64;
                while !stop.load(Relaxed) {
                    for _ in 0..10_000 {
                        x = std::hint::black_box(x * 1.000_000_1 + 1e-9).sqrt() + 0.5;
                    }
                    iters += 1;
                }
                iters
            })
        })
        .collect()
}

/// Peak working set and current private bytes of this process (Windows), or peak RSS
/// (macOS / Unix).
fn memory_bytes() -> (u64, u64) {
    #[cfg(windows)]
    {
        use windows::Win32::System::ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS_EX};
        use windows::Win32::System::Threading::GetCurrentProcess;
        let mut c = PROCESS_MEMORY_COUNTERS_EX { cb: std::mem::size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32, ..Default::default() };
        // SAFETY: valid out pointer and size for the current process.
        let ok = unsafe { GetProcessMemoryInfo(GetCurrentProcess(), &mut c as *mut _ as *mut _, c.cb) };
        if ok.is_ok() {
            return (c.PeakWorkingSetSize as u64, c.PrivateUsage as u64);
        }
        (0, 0)
    }
    #[cfg(not(windows))]
    {
        (0, 0)
    }
}

/// Least-squares fit of `y` on `x`: (slope, residual min, residual max).
fn fit(pts: &[(f64, f64)]) -> (f64, f64, f64) {
    let n = pts.len() as f64;
    let mx = pts.iter().map(|p| p.0).sum::<f64>() / n;
    let my = pts.iter().map(|p| p.1).sum::<f64>() / n;
    let sxy: f64 = pts.iter().map(|p| (p.0 - mx) * (p.1 - my)).sum();
    let sxx: f64 = pts.iter().map(|p| (p.0 - mx).powi(2)).sum();
    let slope = if sxx > 0.0 { sxy / sxx } else { 1.0 };
    let res: Vec<f64> = pts.iter().map(|p| p.1 - (my + slope * (p.0 - mx))).collect();
    (slope, res.iter().cloned().fold(f64::MAX, f64::min), res.iter().cloned().fold(f64::MIN, f64::max))
}

fn cmd_play(a: &PlayArgs) -> Res<()> {
    let master = if a.silence { 0.0f32 } else { 10f64.powf(a.master_db / 20.0) as f32 };
    let params = a.mix.apply(saved_params(&a.song)?);
    let stop_load = Arc::new(AtomicBool::new(false));
    let loaders = busy_threads(a.load_threads, stop_load.clone());
    let engine = Engine::new(EngineConfig { device_id: a.device.clone(), exclusive: a.exclusive, master })?;
    let events = engine.events();
    let loaded = engine.load(&a.song, params)?;
    if a.start > 0.0 {
        engine.seek(a.start);
    }
    let d0 = engine.diagnostics();
    println!("device: {}", d0.device.clone().unwrap_or_else(|| "none".into()));
    println!(
        "song: {} Hz, {} ch, {:.1} s, {} stems; graph latency {} frames; resampling: {}",
        loaded.sample_rate,
        loaded.channels,
        loaded.duration_s,
        loaded.stems.len(),
        d0.graph_latency_frames,
        if d0.device_rate != loaded.sample_rate { format!("{} -> {} Hz", loaded.sample_rate, d0.device_rate) } else { "no".into() }
    );
    println!(
        "master: {}; key {:+}; load threads {}; key change every {} s; seek every {} s; loop {}",
        if master == 0.0 { "digital silence".to_string() } else { db(master as f64) },
        params.semitones,
        a.load_threads,
        a.key_every,
        a.seek_every,
        a.looping
    );
    engine.play();
    let cycle = [3, -2, 5, 0, -7, 12, -12, 2];
    let mut ki = 0usize;
    let mut next_key = if a.key_every > 0.0 { a.key_every } else { f64::MAX };
    let mut next_seek = if a.seek_every > 0.0 { a.seek_every } else { f64::MAX };
    let mut lcg = 0x2545_F491_4F6C_DD1Du64;
    let wall0 = Instant::now();
    // Clock samples per generation: (callback time s, DAC song time s).
    let mut segs: Vec<(u64, Vec<(f64, f64)>)> = Vec::new();
    let mut last_print = 0.0;
    let (mut key_changes, mut seeks, mut loops) = (0, 0, 0);
    let mut peak_ws = 0u64;
    let mut cur = params;
    loop {
        std::thread::sleep(Duration::from_millis(50));
        let w = wall0.elapsed().as_secs_f64();
        while let Ok(ev) = events.try_recv() {
            if let jk_engine::EngineEvent::Error { code, message } = &ev {
                println!("  t={w:7.2} s: engine error {code}: {message}");
            }
            if ev == jk_engine::EngineEvent::DeviceLost {
                println!("  t={w:7.2} s: device lost");
            }
        }
        let snap = engine.snapshot();
        let d = engine.diagnostics();
        if snap.state == PlaybackState::Playing && snap.position_ns != 0 && d.dac_gen == d.generation {
            let t = snap.position_ns as f64 * 1e-9;
            match segs.last_mut() {
                Some((g, v)) if *g == d.generation => {
                    if v.last().is_none_or(|p| p.0 != t) {
                        v.push((t, snap.position_s));
                    }
                }
                _ => segs.push((d.generation, vec![(t, snap.position_s)])),
            }
        }
        if snap.state == PlaybackState::Ended {
            if a.looping {
                engine.seek(0.0);
                engine.play();
                loops += 1;
            } else {
                break;
            }
        }
        if w >= next_key {
            cur = MixParams { semitones: cycle[ki % cycle.len()], ..cur };
            engine.set_params(cur);
            ki += 1;
            key_changes += 1;
            next_key += a.key_every;
        }
        if w >= next_seek {
            lcg = lcg.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            let target = (lcg >> 11) as f64 / (1u64 << 53) as f64 * (loaded.duration_s - 5.0).max(0.0);
            engine.seek(target);
            seeks += 1;
            next_seek += a.seek_every;
        }
        let (pw, _) = memory_bytes();
        peak_ws = peak_ws.max(pw);
        if w - last_print >= 10.0 {
            last_print = w;
            println!(
                "  t={w:7.1} s  pos={:8.3} s  key {:+3}  underruns={} starved={}  lead(min)={}  worker max block {:.2} ms  mem peak {:.1} MB",
                snap.position_s,
                d.audible_semitones,
                d.underruns,
                d.device_starved,
                d.min_lead_ms.map_or("-".into(), |m| format!("{m:.1} ms")),
                d.worker_max_block_ms,
                peak_ws as f64 / 1e6
            );
        }
        if w >= a.seconds {
            break;
        }
    }
    let wall = wall0.elapsed().as_secs_f64();
    let d = engine.diagnostics();
    engine.pause();
    stop_load.store(true, Relaxed);
    let iters: u64 = loaders.into_iter().map(|h| h.join().unwrap_or(0)).sum();
    let (pw, private) = memory_bytes();
    peak_ws = peak_ws.max(pw);

    // Clock vs callback time per continuous segment (skip each segment's first 0.5 s).
    let mut drifts = Vec::new();
    let mut jitter: (f64, f64) = (0.0, 0.0);
    let mut seg_seconds = 0.0;
    for (_, v) in &segs {
        let pts: Vec<(f64, f64)> = v.iter().copied().filter(|p| p.0 - v[0].0 >= 0.5).collect();
        if pts.len() < 40 {
            continue;
        }
        let span = pts.last().unwrap().0 - pts[0].0;
        let (slope, rmin, rmax) = fit(&pts);
        drifts.push(((slope - 1.0) * 1e6, span));
        seg_seconds += span;
        jitter = (jitter.0.min(rmin), jitter.1.max(rmax));
    }
    let wmean = if seg_seconds > 0.0 { drifts.iter().map(|(p, s)| p * s).sum::<f64>() / seg_seconds } else { 0.0 };
    let wmax = drifts.iter().map(|d| d.0.abs()).fold(0.0, f64::max);

    println!("---- results after {wall:.1} s ----");
    println!("underruns: {} callbacks, {} frames of silence inserted", d.underruns, d.underrun_frames);
    println!("device buffer found empty (WASAPI shared mode): {}", d.device_starved);
    println!("device callbacks: {}, longest callback {:.1} us", d.callbacks, d.max_callback_us);
    println!(
        "render worker ({}): {} blocks, CPU {:.2} % of one core, longest block {:.2} ms, min lead {}, reader starved {}, Rubber Band shortfall {} frames, key changes waiting for a pre-built instance {} blocks, instances built on the render thread {}",
        d.worker_priority,
        d.worker_blocks,
        d.worker_busy_s / wall * 100.0,
        d.worker_max_block_ms,
        d.min_lead_ms.map_or("-".into(), |m| format!("{m:.1} ms")),
        d.reader_starved,
        d.shortfall_frames,
        d.deferred_blocks,
        d.inline_constructions
    );
    println!(
        "playback clock (song time at the DAC) vs callback time: {} segments ({seg_seconds:.0} s), drift weighted mean {wmean:+.2} ppm, max |drift| {wmax:.2} ppm, jitter around trend {:+.3} .. {:+.3} ms",
        drifts.len(),
        jitter.0 * 1000.0,
        jitter.1 * 1000.0
    );
    match d.device_drift_ppm {
        Some(ppm) => println!("device clock (IAudioClock) vs OS performance counter: {ppm:+.2} ppm"),
        None => println!("device clock vs OS counter: not available on this backend"),
    }
    println!("events: {key_changes} key changes, {seeks} seeks, {loops} loops");
    println!("memory: peak working set {:.1} MB, private bytes now {:.1} MB", peak_ws as f64 / 1e6, private as f64 / 1e6);
    if a.load_threads > 0 {
        println!("load threads: {}, {iters} work units completed", a.load_threads);
    }
    if let Some(rp) = &a.report {
        let j = serde_json::json!({
            "song": a.song.display().to_string(), "seconds": wall, "device": d.device,
            "underruns": d.underruns, "underrun_frames": d.underrun_frames, "device_starved": d.device_starved,
            "callbacks": d.callbacks, "max_callback_us": d.max_callback_us,
            "worker_cpu_percent": d.worker_busy_s / wall * 100.0, "worker_max_block_ms": d.worker_max_block_ms,
            "min_lead_ms": d.min_lead_ms, "reader_starved": d.reader_starved, "shortfall_frames": d.shortfall_frames,
            "deferred_blocks": d.deferred_blocks, "inline_constructions": d.inline_constructions,
            "drift_ppm_weighted_mean": wmean, "drift_ppm_max_abs": wmax, "drift_segments": drifts.len(),
            "jitter_ms": [jitter.0 * 1000.0, jitter.1 * 1000.0], "device_drift_ppm": d.device_drift_ppm,
            "key_changes": key_changes, "seeks": seeks, "loops": loops, "load_threads": a.load_threads,
            "peak_working_set_bytes": peak_ws, "private_bytes": private,
        });
        std::fs::write(rp, serde_json::to_vec_pretty(&j)?)?;
    }
    drop(engine);
    Ok(())
}

// ---------------------------------------------------------------------------------------
// seek-test

fn cmd_seek_test(song: &Path, seeks: usize, device: Option<String>) -> Res<()> {
    let base = saved_params(song)?;
    let info = jk_format::SongFile::open(song)?;
    let rate = info.manifest().sample_rate;
    let dur = info.manifest().duration();
    drop(info);
    // Reference renders (offline, whole song) for the cases.
    let cases = [
        ("key 0, full mix", MixParams { semitones: 0, ..base }),
        ("key +3, drums only (unshifted path)", MixParams { semitones: 3, guide_vocal: 0.0, backing_on: false, other: 0.0, drums: 1.0, ..base }),
        ("key +3, full mix", MixParams { semitones: 3, ..base }),
    ];
    let tmp = std::env::temp_dir().join(format!("jk-seek-{}.wav", std::process::id()));
    let fade = (rate / 100) as usize;
    let compare = 20_000usize;
    for (name, p) in cases {
        jk_engine::render_offline(song, &p, &tmp, ExportFormat::Wav32f, &mut |_| {}, &AtomicBool::new(false))?;
        let reference: Vec<f32> = hound::WavReader::open(&tmp)?.into_samples::<f32>().collect::<Result<_, _>>()?;
        let (engine, cap) = Engine::new_with_capture(rate, true, 1.0)?;
        engine.load(song, p)?;
        let mut exact = 0;
        let mut worst_diff = f32::MIN;
        let mut rel_dbs: Vec<f64> = Vec::new();
        let mut lcg = 77u64;
        for k in 0..seeks {
            lcg = lcg.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            let t = (lcg >> 11) as f64 / (1u64 << 53) as f64 * (dur - 2.0) + 0.5;
            let frame = (t * rate as f64).round() as usize;
            engine.pause();
            std::thread::sleep(Duration::from_millis(30));
            engine.seek(frame as f64 / rate as f64);
            cap.lock().unwrap().clear();
            engine.play();
            // Wait for enough audio past the fade-in.
            let t0 = Instant::now();
            while cap.lock().unwrap().len() < (fade + compare + 48_000) * 2 && t0.elapsed() < Duration::from_secs(10) {
                std::thread::sleep(Duration::from_millis(5));
            }
            let c = cap.lock().unwrap().clone();
            // Expected: reference from frame+fade on; find it in the capture.
            let want = &reference[(frame + fade) * 2..((frame + fade + compare) * 2).min(reference.len())];
            let mut found = None;
            for d in 0..(c.len() / 2).saturating_sub(want.len() / 2) {
                if c[d * 2] == want[0] && c[d * 2 + 1] == want[1] && c[d * 2..d * 2 + want.len()] == *want {
                    found = Some(d);
                    break;
                }
            }
            match found {
                Some(_) => exact += 1,
                None => {
                    // Not bit-exact (expected with a pitch shift: Rubber Band restarts from
                    // a primed but different internal state). Find the capture offset that
                    // best matches the reference (least squares over the window) and
                    // measure the residual there.
                    let win = want.len().min(16_384);
                    let mut best = (0usize, f64::MAX);
                    for d in 0..(c.len() / 2).saturating_sub(win / 2) {
                        let mut e = 0.0f64;
                        for i in (0..win).step_by(2) {
                            let x = (c[d * 2 + i] - want[i]) as f64;
                            e += x * x;
                            if e >= best.1 {
                                break;
                            }
                        }
                        if e < best.1 {
                            best = (d, e);
                        }
                    }
                    let d = best.0;
                    let n = want.len().min(c.len().saturating_sub(d * 2));
                    let (mut se, mut ss) = (0.0f64, 0.0f64);
                    for i in 0..n {
                        let x = (c[d * 2 + i] - want[i]) as f64;
                        se += x * x;
                        ss += (want[i] as f64).powi(2);
                    }
                    let rel_db = 10.0 * (se / ss.max(1e-30)).log10();
                    worst_diff = worst_diff.max(rel_db as f32);
                    rel_dbs.push(rel_db);
                }
            }
            let _ = k;
        }
        println!(
            "seek accuracy, {name}: {exact}/{seeks} seeks bit-exact against the offline render for {compare} frames after the 10 ms fade-in{}",
            if exact < seeks {
                rel_dbs.sort_by(f64::total_cmp);
                format!(
                    "; the others, at their best (least-squares) alignment: residual {:.1} dB (median) .. {worst_diff:.1} dB (worst) relative to the signal",
                    rel_dbs[rel_dbs.len() / 2]
                )
            } else {
                String::new()
            }
        );
        drop(engine);
    }
    let _ = std::fs::remove_file(&tmp);

    // Latency on a real device (digital silence).
    let engine = Engine::new(EngineConfig { device_id: device, exclusive: false, master: 0.0 })?;
    engine.load(song, base)?;
    engine.play();
    std::thread::sleep(Duration::from_millis(500));
    let mut to_adopt = Vec::new();
    let mut to_dac = Vec::new();
    let mut lcg = 99u64;
    for _ in 0..seeks.max(20) {
        lcg = lcg.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        let t = (lcg >> 11) as f64 / (1u64 << 53) as f64 * (dur - 3.0) + 0.5;
        let t0 = Instant::now();
        engine.seek(t);
        let g = engine.diagnostics().generation;
        let mut adopted = None;
        loop {
            let d = engine.diagnostics();
            if d.dac_gen == g {
                if adopted.is_none() {
                    adopted = Some(t0.elapsed().as_secs_f64());
                }
                if d.dac_pos > t + 1e-6 {
                    to_dac.push(t0.elapsed().as_secs_f64() * 1000.0);
                    break;
                }
            }
            if t0.elapsed() > Duration::from_secs(3) {
                break;
            }
            std::thread::sleep(Duration::from_micros(500));
        }
        if let Some(a) = adopted {
            to_adopt.push(a * 1000.0);
        }
        std::thread::sleep(Duration::from_millis(300));
    }
    let d = engine.diagnostics();
    let stats = |v: &mut Vec<f64>| {
        v.sort_by(f64::total_cmp);
        if v.is_empty() {
            return "n/a".to_string();
        }
        format!("median {:.1} ms, min {:.1}, max {:.1} (n={})", v[v.len() / 2], v[0], v[v.len() - 1], v.len())
    };
    println!("device: {}", d.device.unwrap_or_default());
    println!("seek latency, call -> new position at the DAC clock (silence-to-audio at the speaker): {}", stats(&mut to_dac));
    println!("seek latency, call -> filler adopts the new segment: {}", stats(&mut to_adopt));
    println!("underruns during the seek test: {}", d.underruns);
    Ok(())
}

// ---------------------------------------------------------------------------------------
// gen-test-stems / render-stems

/// f32 -> 24-bit integer with the same scale the decoder uses (symphonia: `x / 2^23`), so
/// every value on the 24-bit grid round-trips exactly. (+1.0 itself is not representable
/// and clamps to 8 388 607; `write_flac24` refuses peaks >= 1.0 anyway.)
fn to_i24(x: f32) -> i32 {
    (x as f64 * 8_388_608.0).round().clamp(-8_388_608.0, 8_388_607.0) as i32
}

fn write_flac24(path: &Path, ch: &[Vec<f32>; 2], rate: u32) -> Res<f32> {
    let mut peak = 0.0f32;
    let mut samples = Vec::with_capacity(ch[0].len() * 2);
    for i in 0..ch[0].len() {
        for c in ch {
            peak = peak.max(c[i].abs());
            samples.push(to_i24(c[i]));
        }
    }
    if peak >= 1.0 {
        return Err(format!("{}: peak {peak} would clip", path.display()).into());
    }
    write_atomic(path, &jk_engine::encode_flac(&samples, 2, 24, rate))?;
    Ok(peak)
}

/// Write via a temporary file + rename (WORK_ORDER trap 13).
fn write_atomic(path: &Path, bytes: &[u8]) -> Res<()> {
    let tmp = path.with_extension("tmp-write");
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

fn gen_test_stems(out: Option<PathBuf>, seconds: f64, rate: u32, seed: u32) -> Res<()> {
    let out = out.unwrap_or_else(|| jk_build().join("fixtures").join(format!("synth-{rate}")));
    std::fs::create_dir_all(&out)?;
    let t = Instant::now();
    let stems = jk_dsp::signals::synth_song(rate, seconds, seed);
    let names = ["lead", "backing", "drums", "other"];
    for (i, name) in names.iter().enumerate() {
        let p = out.join(format!("{name}.flac"));
        let peak = write_flac24(&p, &stems[i], rate)?;
        println!("{}: {:.1} s, 24-bit FLAC, peak {:.2} dBFS", p.display(), seconds, 20.0 * peak.log10());
    }
    let mix: [Vec<f32>; 2] = std::array::from_fn(|c| {
        (0..stems[0][0].len()).map(|i| stems.iter().map(|s| s[c][i]).sum::<f32>()).collect()
    });
    let p = out.join("mix.flac");
    let peak = write_flac24(&p, &mix, rate)?;
    println!("{}: sum of the stems at unity, peak {:.2} dBFS", p.display(), 20.0 * peak.log10());
    std::fs::write(
        out.join("SOURCE.txt"),
        format!(
            "Generated by jukeoke-cli gen-test-stems (jk_dsp::signals::synth_song, seed {seed}, {rate} Hz, {seconds} s).\n\
             Entirely synthetic (sines, noise): no copyrighted material. Licence: same as the project (GPL-3.0-or-later).\n"
        ),
    )?;
    println!("done in {:.1} s", t.elapsed().as_secs_f64());
    Ok(())
}

fn load_stems(dir: &Path) -> Res<(u32, [[Vec<f32>; 2]; 4])> {
    let mut rate = 0;
    let mut out: Vec<[Vec<f32>; 2]> = Vec::new();
    for n in ["lead", "backing", "drums", "other"] {
        let (r, ch) = FlacStem::decode_all(&dir.join(format!("{n}.flac")))?;
        if rate != 0 && r != rate {
            return Err("stems have different sample rates".into());
        }
        rate = r;
        out.push(ch);
    }
    let len = out.iter().map(|c| c[0].len()).max().unwrap_or(0);
    for c in out.iter_mut() {
        c[0].resize(len, 0.0);
        c[1].resize(len, 0.0);
    }
    let arr: [[Vec<f32>; 2]; 4] = out.try_into().map_err(|_| "need four stems")?;
    Ok((rate, arr))
}

fn write_wav_f32(path: &Path, ch: &[Vec<f32>; 2], rate: u32) -> Res<()> {
    let spec = hound::WavSpec { channels: 2, sample_rate: rate, bits_per_sample: 32, sample_format: hound::SampleFormat::Float };
    let tmp = path.with_extension("tmp-write");
    {
        let mut w = hound::WavWriter::create(&tmp, spec)?;
        for i in 0..ch[0].len() {
            w.write_sample(ch[0][i])?;
            w.write_sample(ch[1][i])?;
        }
        w.finalize()?;
    }
    std::fs::rename(&tmp, path)?;
    Ok(())
}

fn cmd_render_stems(stems_dir: &Path, out: &Path, rate: Option<u32>, params: DspParams) -> Res<()> {
    let t = Instant::now();
    let (sr, stems) = load_stems(stems_dir)?;
    let frames = stems[0][0].len();
    let sref = StemsRef { sample_rate: sr, frames, stems: std::array::from_fn(|s| [&stems[s][0][..], &stems[s][1][..]]) };
    let mut opts = RenderOptions::native(sr);
    opts.out_rate = rate.unwrap_or(sr);
    let t_dec = t.elapsed().as_secs_f64();
    let r = render(sref, params, &[], &opts)?;
    let t_ren = t.elapsed().as_secs_f64() - t_dec;
    write_wav_f32(out, &r.channels, r.sample_rate)?;
    let peak = r.channels.iter().flat_map(|c| c.iter()).fold(0.0f32, |m, v| m.max(v.abs()));
    println!(
        "{}: {} frames at {} Hz, 32-bit float, peak {:.2} dBFS; key {:+}, graph latency {} frames removed, resampler {}",
        out.display(),
        r.channels[0].len(),
        r.sample_rate,
        20.0 * peak.log10(),
        params.semitones,
        r.graph_latency,
        if r.sample_rate == sr { "not used".to_string() } else { format!("{sr} -> {} Hz (delay {} removed)", r.sample_rate, r.resampler_delay) }
    );
    println!("decode {t_dec:.2} s, render {t_ren:.2} s ({:.1}x real time)", frames as f64 / sr as f64 / t_ren);
    Ok(())
}

// ---------------------------------------------------------------------------------------
// ci-render

/// The cross-platform render set. Inputs are generated with `jk_dsp::signals` (pure
/// arithmetic + the `libm` crate), so both platforms start from bit-identical stems.
fn cmd_ci_render(out: &Path) -> Res<()> {
    std::fs::create_dir_all(out)?;
    let mut cases: Vec<(String, u32, u32, DspParams, Vec<ParamEvent>)> = Vec::new();
    let base = DspParams { guide: 0.3, ..DspParams::default() };
    for s in [-12, -7, -5, -3, -2, -1, 0, 1, 2, 3, 5, 7, 12] {
        cases.push((format!("key_{s:+03}.wav"), 44_100, 44_100, DspParams { semitones: s, ..base }, vec![]));
    }
    for s in [-5, 3] {
        cases.push((format!("key_{s:+03}_shiftdrums.wav"), 44_100, 44_100, DspParams { semitones: s, shift_drums: true, ..base }, vec![]));
    }
    let r = 44_100u64;
    cases.push((
        "keychanges_0_p2_m3_0.wav".into(),
        44_100,
        44_100,
        base,
        vec![
            ParamEvent { frame: r, params: DspParams { semitones: 2, ..base } },
            ParamEvent { frame: 2 * r, params: DspParams { semitones: -3, backing: 0.5, ..base } },
            ParamEvent { frame: 3 * r, params: DspParams { semitones: 0, ..base } },
        ],
    ));
    cases.push(("resample_44k1_to_48k_key_+02.wav".into(), 44_100, 48_000, DspParams { semitones: 2, ..base }, vec![]));
    cases.push(("song48k_key_+04.wav".into(), 48_000, 48_000, DspParams { semitones: 4, ..base }, vec![]));
    let seconds = 4.0;
    let mut songs: std::collections::HashMap<u32, [[Vec<f32>; 2]; 4]> = std::collections::HashMap::new();
    for (name, song_rate, out_rate, params, events) in &cases {
        let stems = songs.entry(*song_rate).or_insert_with(|| jk_dsp::signals::synth_song(*song_rate, seconds, 21));
        let frames = stems[0][0].len();
        let sref = StemsRef { sample_rate: *song_rate, frames, stems: std::array::from_fn(|s| [&stems[s][0][..], &stems[s][1][..]]) };
        let mut opts = RenderOptions::native(*song_rate);
        opts.out_rate = *out_rate;
        let o = render(sref, *params, events, &opts)?;
        if o.stats.shift_shortfall_frames != 0 {
            return Err(format!("{name}: Rubber Band shortfall").into());
        }
        write_wav_f32(&out.join(name), &o.channels, o.sample_rate)?;
        println!("{name}: {} frames at {} Hz", o.channels[0].len(), o.sample_rate);
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------
// bench

fn cmd_bench(seconds: f64) -> Res<()> {
    use jk_rubberband::{semitones_to_scale, Stretcher, StretcherConfig};
    println!("CPU: real-time factor = processing time / audio duration (lower is better); stereo; {seconds} s of synthetic music per run");
    if cfg!(debug_assertions) {
        println!("WARNING: debug build of the Rust code; run with --release for real numbers");
    }
    for &rate in &[44_100u32, 48_000] {
        let stems = jk_dsp::signals::synth_song(rate, seconds, 5);
        let frames = stems[0][0].len();
        let mix: [Vec<f32>; 2] = std::array::from_fn(|c| (0..frames).map(|i| stems.iter().map(|s| s[c][i]).sum::<f32>()).collect());
        for &semis in &[-12i32, -5, -2, 2, 5, 12] {
            let mut cfg = StretcherConfig::new(rate, 2);
            cfg.max_process_size = 256;
            let mut s = Stretcher::new(cfg)?;
            s.set_pitch_scale(semitones_to_scale(semis as f64));
            let mut ol = vec![0f32; 8192];
            let mut or = vec![0f32; 8192];
            let t = Instant::now();
            for (cl, cr) in mix[0].chunks(256).zip(mix[1].chunks(256)) {
                s.process(&[cl, cr], cl.len(), false);
                while let Some(a) = s.available() {
                    if a == 0 {
                        break;
                    }
                    let a = a.min(8192);
                    let mut o: [&mut [f32]; 2] = [&mut ol[..a], &mut or[..a]];
                    s.retrieve(&mut o);
                }
            }
            let el = t.elapsed().as_secs_f64();
            println!("Rubber Band R3 alone   {rate} Hz {semis:+3} st: RTF {:.4} ({:.0}x real time)", el / seconds, seconds / el);
        }
        // Whole graph through the offline renderer (mix + pitch + drum delay), no resampling.
        let sref = StemsRef { sample_rate: rate, frames, stems: std::array::from_fn(|s| [&stems[s][0][..], &stems[s][1][..]]) };
        for &semis in &[0i32, 3] {
            let t = Instant::now();
            let _ = render(sref, DspParams { semitones: semis, ..DspParams::default() }, &[] as &[ParamEvent], &RenderOptions::native(rate))?;
            let el = t.elapsed().as_secs_f64();
            println!("full graph (render)    {rate} Hz {semis:+3} st: RTF {:.4} ({:.0}x real time)", el / seconds, seconds / el);
        }
        if rate == 44_100 {
            let t = Instant::now();
            let mut o = RenderOptions::native(rate);
            o.out_rate = 48_000;
            let _ = render(sref, DspParams { semitones: 3, ..DspParams::default() }, &[], &o)?;
            let el = t.elapsed().as_secs_f64();
            println!("full graph + resampler 44100->48000 +3 st: RTF {:.4} ({:.0}x real time)", el / seconds, seconds / el);
        }
    }
    // Building one Rubber Band instance (what the house thread does before a key change).
    let t = Instant::now();
    let n = 20;
    for _ in 0..n {
        let s = Stretcher::new(jk_dsp::stretcher_config(44_100))?;
        drop(s);
    }
    println!("Rubber Band R3 construct + drop at 44.1 kHz: {:.2} ms each (off the render thread)", t.elapsed().as_secs_f64() * 1000.0 / n as f64);
    Ok(())
}

// ---------------------------------------------------------------------------------------
// key-switch-capture (Work Order 2: the key in effect)

#[allow(clippy::too_many_arguments)]
fn cmd_key_switch_capture(
    song: &Path,
    out: &Path,
    mode: &str,
    cache: Option<PathBuf>,
    start: f64,
    changes: &str,
    seconds: f64,
    full_mix: bool,
) -> Res<()> {
    use jk_engine::KeyMode;
    let plan: Vec<(f64, i32)> = changes
        .split(',')
        .filter(|x| !x.trim().is_empty())
        .map(|x| {
            let (t, k) = x.split_once(':').ok_or("changes: expected seconds:key")?;
            Ok::<_, Box<dyn std::error::Error>>((t.trim().parse()?, k.trim().parse()?))
        })
        .collect::<Result<_, _>>()?;
    std::fs::create_dir_all(out)?;
    let rate = jk_format::SongFile::open(song)?.manifest().sample_rate;
    // The null device at the song rate: no resampling, nothing audible, real-time pacing.
    let (engine, cap) = Engine::new_with_capture(rate, false, 1.0)?;
    match mode {
        "realtime" => engine.set_key_mode(KeyMode::Realtime),
        "prerender" => {
            let dir = cache.unwrap_or_else(|| out.join("keycache"));
            engine.set_key_cache(Some(&dir), 8 << 30)?;
            engine.set_key_mode(KeyMode::Prerender);
        }
        m => return Err(format!("unknown mode {m}").into()),
    }
    let base = if full_mix {
        MixParams { guide_vocal: 1.0, ..MixParams::default() }
    } else {
        MixParams { guide_vocal: 1.0, backing_on: false, other: 0.0, drums: 0.0, ..MixParams::default() }
    };
    engine.load(song, base)?;
    engine.seek(start);
    let t = Instant::now();
    while engine.key_at(start, Some(engine.snapshot().key.segment)).is_none() {
        if t.elapsed() > Duration::from_secs(5) {
            return Err("the engine did not start the segment".into());
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    cap.lock().map_err(|_| "capture lock")?.clear();
    engine.play();
    let t0 = Instant::now();
    let mut next = 0;
    let mut requests = Vec::new();
    // Per change: first wall time its switch was published with a position, and first
    // wall time it was heard at the DAC.
    let mut published: Vec<Option<f64>> = vec![None; plan.len()];
    let mut heard: Vec<Option<f64>> = vec![None; plan.len()];
    while t0.elapsed().as_secs_f64() < seconds {
        let now = t0.elapsed().as_secs_f64();
        if next < plan.len() && now >= plan[next].0 {
            let s = engine.snapshot();
            requests.push(serde_json::json!({ "wall_s": now, "key": plan[next].1, "position_s": s.position_s }));
            engine.set_params(MixParams { semitones: plan[next].1, ..base });
            next += 1;
        }
        let s = engine.snapshot();
        for i in 0..next {
            if published[i].is_none()
                && s.key.requested == plan[i].1
                && s.key.pending.is_some_and(|p| p.semitones == plan[i].1 && p.at_s.is_some())
            {
                published[i] = Some(now);
            }
            if heard[i].is_none() && s.key.heard == plan[i].1 && i + 1 == next {
                heard[i] = Some(now);
            }
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    engine.pause();
    let snap = engine.snapshot();
    let d = engine.diagnostics();
    let timeline = engine.key_timeline();
    let seg = timeline.iter().find(|x| x.segment == snap.key.segment).ok_or("no key timeline")?;
    let samples = cap.lock().map_err(|_| "capture lock")?.clone();
    let k0 = samples.chunks(2).position(|f| f[0] != 0.0 || f[1] != 0.0).ok_or("nothing was played")?;
    let left: Vec<f32> = samples.chunks(2).map(|f| f[0]).collect();
    let right: Vec<f32> = samples.chunks(2).map(|f| f[1]).collect();
    write_wav_f32(&out.join("capture.wav"), &[left, right], rate)?;
    let report = serde_json::json!({
        "song": song.file_name().map(|x| x.to_string_lossy().to_string()),
        "mode": mode,
        "key_mode_in_effect": format!("{:?}", snap.key.mode),
        "rate": rate,
        "start_s": start,
        "first_frame_index": k0,
        "note": "first_frame_index: the first non-zero captured frame. Playback starts at song frame round(start_s * rate) after digital silence, so this is that frame unless the lead vocal itself is silent there (then align by content). The capture is contiguous when underruns == 0.",
        "underruns": d.underruns,
        "key_log_lost": d.key_log_lost,
        "cache_misses": d.cache_misses,
        "requests": requests,
        "published_wall_s": published,
        "heard_wall_s": heard,
        "segment": seg.segment,
        "keys": seg.keys,
    });
    std::fs::write(out.join("keys.json"), serde_json::to_string_pretty(&report)?)?;
    println!("{}: {} frames captured, underruns {}, key changes published: {:?}", out.display(), samples.len() / 2, d.underruns, &seg.keys[1..]);
    if d.underruns > 0 {
        return Err("underruns: the capture is not contiguous".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    /// Values on the 24-bit grid (what the decoder produces) must encode to the same
    /// integer, so decode -> encode is lossless.
    #[test]
    fn to_i24_round_trips_the_24_bit_grid() {
        for k in [-8_388_608i32, -6_291_456, -4_194_305, -1, 0, 1, 4_194_305, 6_291_456, 8_388_607] {
            let x = k as f32 / 8_388_608.0;
            assert_eq!(super::to_i24(x), k, "grid value {k}");
        }
        assert_eq!(super::to_i24(1.0), 8_388_607);
        assert_eq!(super::to_i24(-1.0), -8_388_608);
    }
}

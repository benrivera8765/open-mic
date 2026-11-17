// SPDX-License-Identifier: GPL-3.0-or-later
//! `jukeoke-cli`: developer tool for the audio engine.
//!
//! * `gen-test-stems`: synthetic 24-bit FLAC stems + their sum (no copyrighted material).
//! * `render`: offline render of a stem folder to a 32-bit float WAV.
//! * `play`: play a stem folder through the default device for N seconds and print
//!   device format, reported latency, clock drift and underruns.
//! * `bench`: CPU cost (real-time factor) of Rubber Band R3 and the whole graph.
//! * `ci-render`: fixed set of offline renders of generated material, 32-bit float WAV,
//!   for the Windows-vs-macOS comparison in CI (scripts/ci-render.sh,
//!   scripts/ci-compare-renders.sh).

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering::Relaxed};
use std::sync::Arc;
use std::time::{Duration, Instant};

use clap::{Args, Parser, Subcommand};
use jk_dsp::{render, MixParams, ParamEvent, RenderOptions, StemsRef};
use jk_engine::{FlacStem, OutputRequest, Player, PlayerConfig, StemFiles};

type Res<T> = Result<T, Box<dyn std::error::Error>>;

#[derive(Parser)]
#[command(name = "jukeoke-cli", version, about = "Jukeoke audio engine developer tool")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
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
    /// Offline render of a stem folder to a 32-bit float WAV.
    Render {
        /// Folder with lead.flac, backing.flac, drums.flac, other.flac.
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
    /// Play through the default output device and report timing statistics.
    Play {
        #[arg(long)]
        stems: PathBuf,
        #[arg(long, default_value_t = 10.0)]
        seconds: f64,
        /// Master gain in dB (default -60 dB: inaudible test level).
        #[arg(long, default_value_t = -60.0, allow_hyphen_values = true)]
        master_db: f64,
        /// Play digital silence (master gain 0).
        #[arg(long)]
        silence: bool,
        /// Allow a master gain above -60 dB (the default refuses, so a test run can never
        /// be loud by accident).
        #[arg(long)]
        loud: bool,
        /// WASAPI exclusive mode.
        #[arg(long)]
        exclusive: bool,
        /// Busy-loop threads to run in parallel (CPU load test).
        #[arg(long, default_value_t = 0)]
        load_threads: usize,
        /// Change key every N seconds through +3, -2, +5, 0, -7, ... (0 = off).
        #[arg(long, default_value_t = 0.0)]
        key_cycle: f64,
        #[arg(long, default_value_t = 0.0)]
        start: f64,
        #[command(flatten)]
        mix: MixArgs,
    },
    /// Measure the CPU cost of Rubber Band R3 and of the full graph (run with --release).
    Bench {
        #[arg(long, default_value_t = 30.0)]
        seconds: f64,
    },
    /// Render a fixed set of generated test cases to OUT_DIR (CI cross-platform check).
    CiRender {
        out_dir: PathBuf,
    },
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
    fn params(&self) -> MixParams {
        MixParams {
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
        Cmd::GenTestStems { out, seconds, rate, seed } => gen_test_stems(out, seconds, rate, seed),
        Cmd::Render { stems, out, rate, mix } => cmd_render(&stems, &out, rate, mix.params()),
        Cmd::Play { stems, seconds, master_db, silence, loud, exclusive, load_threads, key_cycle, start, mix } => {
            if !silence && !loud && master_db > -60.0 {
                Err("master gain above -60 dB needs --loud".into())
            } else {
                cmd_play(&stems, seconds, if silence { None } else { Some(master_db) }, exclusive, load_threads, key_cycle, start, mix.params())
            }
        }
        Cmd::Bench { seconds } => cmd_bench(seconds),
        Cmd::CiRender { out_dir } => cmd_ci_render(&out_dir),
    };
    if let Err(e) = r {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

fn jk_build() -> PathBuf {
    std::env::var_os("JK_BUILD").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("../jukeoke-build"))
}

// ---------------------------------------------------------------------------------------
// gen-test-stems

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

// ---------------------------------------------------------------------------------------
// render

fn load_stems(dir: &Path) -> Res<(u32, [[Vec<f32>; 2]; 4])> {
    let files = StemFiles::in_dir(dir);
    let mut rate = 0;
    let mut out: Vec<[Vec<f32>; 2]> = Vec::new();
    for p in &files.paths {
        let (r, ch) = FlacStem::decode_all(p)?;
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

fn cmd_render(stems_dir: &Path, out: &Path, rate: Option<u32>, params: MixParams) -> Res<()> {
    let t = Instant::now();
    let (sr, stems) = load_stems(stems_dir)?;
    let frames = stems[0][0].len();
    let sref = StemsRef { sample_rate: sr, frames, stems: std::array::from_fn(|s| [&stems[s][0][..], &stems[s][1][..]]) };
    let mut opts = RenderOptions::native(sr);
    opts.out_rate = rate.unwrap_or(sr);
    let t_dec = t.elapsed().as_secs_f64();
    let r = render(sref, params, &[], &opts)?;
    let t_ren = t.elapsed().as_secs_f64() - t_dec;
    let spec = hound::WavSpec { channels: 2, sample_rate: r.sample_rate, bits_per_sample: 32, sample_format: hound::SampleFormat::Float };
    let tmp = out.with_extension("tmp-write");
    {
        let mut w = hound::WavWriter::create(&tmp, spec)?;
        for i in 0..r.channels[0].len() {
            w.write_sample(r.channels[0][i])?;
            w.write_sample(r.channels[1][i])?;
        }
        w.finalize()?;
    }
    std::fs::rename(&tmp, out)?;
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

#[allow(clippy::too_many_arguments)]
fn cmd_play(
    stems_dir: &Path,
    seconds: f64,
    master_db: Option<f64>,
    exclusive: bool,
    load_threads: usize,
    key_cycle: f64,
    start_s: f64,
    params: MixParams,
) -> Res<()> {
    let master = match master_db {
        None => 0.0f32,
        Some(db) => 10f64.powf(db / 20.0) as f32,
    };
    let files = StemFiles::in_dir(stems_dir);
    let probe = FlacStem::open(&files.paths[0])?;
    let start_frame = (start_s * probe.sample_rate() as f64) as u64;
    drop(probe);
    let cfg = PlayerConfig {
        output: OutputRequest { exclusive, preferred_rate: None, buffer_ms: 20 },
        start_frame,
        params,
        master_gain: master,
        ..PlayerConfig::default()
    };
    let stop_load = Arc::new(AtomicBool::new(false));
    let loaders = busy_threads(load_threads, stop_load.clone());
    let mut player = Player::start(&files, cfg)?;
    let d = player.device.clone();
    println!("device: {} [{} {}]", d.name, d.backend, d.share_mode);
    println!("device format: {} Hz, {} ch, {}, endpoint buffer {} frames ({:.2} ms)", d.sample_rate, d.channels, d.sample_format, d.buffer_frames, d.buffer_frames as f64 * 1000.0 / d.sample_rate as f64);
    println!("reported stream latency: {} frames ({:.2} ms)", d.stream_latency_frames, d.stream_latency_frames as f64 * 1000.0 / d.sample_rate as f64);
    println!(
        "song: {} Hz, {:.1} s; graph latency {} frames ({:.1} ms); resampling: {}",
        player.song_rate,
        player.song_frames as f64 / player.song_rate as f64,
        player.graph_latency,
        player.graph_latency as f64 * 1000.0 / player.song_rate as f64,
        if player.resampling { format!("yes, {} -> {} Hz (rubato sinc, delay {} frames)", player.song_rate, d.sample_rate, player.resampler_delay) } else { "no (rates match)".into() }
    );
    println!("master gain: {}; key {:+}; load threads: {load_threads}", if master == 0.0 { "digital silence".to_string() } else { format!("{:.1} dB", 20.0 * (master as f64).log10()) }, params.semitones);

    let clock = player.clock.clone();
    // Wait for the stream to start (first callback).
    let t_wait = Instant::now();
    while clock.callbacks.load(Relaxed) == 0 {
        if t_wait.elapsed() > Duration::from_secs(5) {
            return Err("device did not start within 5 s".into());
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    let wall0 = Instant::now();
    let song0 = clock.song_time_at_dac();
    let cycle = [3, -2, 5, 0, -7, 12, -12, 2];
    let mut next_key = key_cycle;
    let mut ki = 0;
    let mut samples: Vec<(f64, f64)> = Vec::new(); // (wall, clock - wall), raw clock
    let mut isamples: Vec<(f64, f64)> = Vec::new(); // same, interpolated clock
    let isong0 = clock.song_time_interpolated(jk_engine::clock::now_ns());
    let mut last_print = 0.0;
    loop {
        std::thread::sleep(Duration::from_millis(50));
        let w = wall0.elapsed().as_secs_f64();
        let s = clock.song_time_at_dac() - song0;
        samples.push((w, s - w));
        let si = clock.song_time_interpolated(jk_engine::clock::now_ns()) - isong0;
        isamples.push((w, si - w));
        if key_cycle > 0.0 && w >= next_key {
            let p = MixParams { semitones: cycle[ki % cycle.len()], ..params };
            player.set_params(p);
            println!("  t={w:6.2} s: key -> {:+}", p.semitones);
            ki += 1;
            next_key += key_cycle;
        }
        if w - last_print >= 5.0 {
            last_print = w;
            println!(
                "  t={w:6.2} s  clock={:8.3} s  clock-wall={:+7.2} ms  underruns={}  ahead(min)={:.1} ms",
                s,
                (s - w) * 1000.0,
                clock.underruns.load(Relaxed),
                player.stats.min_ahead_frames.load(Relaxed) as f64 * 1000.0 / d.sample_rate as f64
            );
        }
        if w >= seconds || player.is_finished() {
            break;
        }
    }
    let wall = wall0.elapsed().as_secs_f64();
    stop_load.store(true, Relaxed);
    let iters: u64 = loaders.into_iter().map(|h| h.join().unwrap_or(0)).sum();

    // Clock vs wall: least-squares slope (ppm) and spread after removing the slope.
    let fit = |samples: &[(f64, f64)]| {
        let n = samples.len() as f64;
        let (mx, my) = (samples.iter().map(|s| s.0).sum::<f64>() / n, samples.iter().map(|s| s.1).sum::<f64>() / n);
        let sxy: f64 = samples.iter().map(|s| (s.0 - mx) * (s.1 - my)).sum();
        let sxx: f64 = samples.iter().map(|s| (s.0 - mx).powi(2)).sum();
        let slope = if sxx > 0.0 { sxy / sxx } else { 0.0 };
        let resid: Vec<f64> = samples.iter().map(|s| s.1 - (my + slope * (s.0 - mx))).collect();
        let rmin = resid.iter().cloned().fold(f64::MAX, f64::min);
        let rmax = resid.iter().cloned().fold(f64::MIN, f64::max);
        let last = samples.last().map(|s| s.1).unwrap_or(0.0);
        (slope, rmin, rmax, last)
    };
    let (slope, rmin, rmax, last) = fit(&samples);
    let (islope, irmin, irmax, ilast) = fit(&isamples);

    let st = &player.stats;
    let busy = st.busy_ns.load(Relaxed) as f64 * 1e-9;
    println!("---- results after {wall:.2} s wall time ----");
    println!("underruns: {} callbacks, {} frames of silence inserted", clock.underruns.load(Relaxed), clock.underrun_frames.load(Relaxed));
    println!(
        "device buffer found empty (WASAPI shared mode: device thread late, likely gap): {}",
        clock.device_starved.load(Relaxed)
    );
    println!("device callbacks: {}, longest callback {:.1} us", clock.callbacks.load(Relaxed), clock.max_callback_ns.load(Relaxed) as f64 / 1000.0);
    println!(
        "render worker ({}): {} blocks, CPU {:.2} % of one core, longest block {:.2} ms, min lead {:.1} ms, reader starved {} times, Rubber Band shortfall {} frames",
        st.worker_priority.get().map(|s| s.as_str()).unwrap_or("?"),
        st.blocks.load(Relaxed),
        busy / wall * 100.0,
        st.max_block_ns.load(Relaxed) as f64 / 1e6,
        st.min_ahead_frames.load(Relaxed) as f64 * 1000.0 / d.sample_rate as f64,
        st.reader_starved.load(Relaxed),
        st.graph_shortfall_frames.load(Relaxed),
    );
    println!(
        "playback clock (frames delivered - reported latency) vs wall clock: end offset {:+.3} ms, trend {:+.1} ppm, jitter around trend {:+.3} .. {:+.3} ms ({} samples every 50 ms)",
        last * 1000.0,
        slope * 1e6,
        rmin * 1000.0,
        rmax * 1000.0,
        samples.len()
    );
    println!(
        "same, interpolated between callbacks (what the UI does): end offset {:+.3} ms, trend {:+.1} ppm, jitter {:+.3} .. {:+.3} ms",
        ilast * 1000.0,
        islope * 1e6,
        irmin * 1000.0,
        irmax * 1000.0
    );
    match clock.device_drift_ppm() {
        Some(ppm) => println!("device clock (IAudioClock position) vs OS performance counter: {ppm:+.2} ppm"),
        None => println!("device clock vs OS counter: not available on this backend"),
    }
    if load_threads > 0 {
        println!("load threads: {load_threads}, {iters} work units completed");
    }
    player.stop()?;
    Ok(())
}

// ---------------------------------------------------------------------------------------
// ci-render

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

/// The cross-platform render set. Inputs are generated with `jk_dsp::signals` (pure
/// arithmetic + the `libm` crate), so both platforms start from bit-identical stems.
fn cmd_ci_render(out: &Path) -> Res<()> {
    std::fs::create_dir_all(out)?;
    let mut cases: Vec<(String, u32, u32, MixParams, Vec<ParamEvent>)> = Vec::new();
    let base = MixParams { guide: 0.3, ..MixParams::default() };
    for s in [-12, -7, -5, -3, -2, -1, 0, 1, 2, 3, 5, 7, 12] {
        cases.push((format!("key_{s:+03}.wav"), 44_100, 44_100, MixParams { semitones: s, ..base }, vec![]));
    }
    for s in [-5, 3] {
        cases.push((format!("key_{s:+03}_shiftdrums.wav"), 44_100, 44_100, MixParams { semitones: s, shift_drums: true, ..base }, vec![]));
    }
    let r = 44_100u64;
    cases.push((
        "keychanges_0_p2_m3_0.wav".into(),
        44_100,
        44_100,
        base,
        vec![
            ParamEvent { frame: r, params: MixParams { semitones: 2, ..base } },
            ParamEvent { frame: 2 * r, params: MixParams { semitones: -3, backing: 0.5, ..base } },
            ParamEvent { frame: 3 * r, params: MixParams { semitones: 0, ..base } },
        ],
    ));
    cases.push(("resample_44k1_to_48k_key_+02.wav".into(), 44_100, 48_000, MixParams { semitones: 2, ..base }, vec![]));
    cases.push(("song48k_key_+04.wav".into(), 48_000, 48_000, MixParams { semitones: 4, ..base }, vec![]));
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
            let _ = render(sref, MixParams { semitones: semis, ..MixParams::default() }, &[] as &[ParamEvent], &RenderOptions::native(rate))?;
            let el = t.elapsed().as_secs_f64();
            println!("full graph (render)    {rate} Hz {semis:+3} st: RTF {:.4} ({:.0}x real time)", el / seconds, seconds / el);
        }
        if rate == 44_100 {
            let t = Instant::now();
            let mut o = RenderOptions::native(rate);
            o.out_rate = 48_000;
            let _ = render(sref, MixParams { semitones: 3, ..MixParams::default() }, &[], &o)?;
            let el = t.elapsed().as_secs_f64();
            println!("full graph + resampler 44100->48000 +3 st: RTF {:.4} ({:.0}x real time)", el / seconds, seconds / el);
        }
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

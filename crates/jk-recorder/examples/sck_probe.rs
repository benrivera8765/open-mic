// SPDX-License-Identifier: GPL-3.0-or-later
//! `sck_probe`: a three-second ScreenCaptureKit self-check for CI and for the
//! owner's MacBook (WORK_ORDER sections 4.10 and 8, Stage 0 experiment 3).
//!
//! ```text
//! cargo run --release -p jk-recorder --example sck_probe -- [options]
//!
//!   --seconds N      capture length (default 3)
//!   --out FILE.wav   where to write the capture (default: a temp file, deleted
//!                    afterwards unless --keep)
//!   --keep           keep the capture file
//!   --request        if Screen Recording is not allowed yet, ask macOS (may
//!                    show the system prompt; without this flag the probe never
//!                    prompts)
//!   --tone           play a 997 Hz tone at -20 dBFS with /usr/bin/afplay during
//!                    the capture and check that it comes back
//!   --report FILE    also write the plain-text report to FILE
//! ```
//!
//! It prints the macOS version, the output device and its format, the
//! permission state, the real capture format, sample count against the OS
//! clock span of the buffers (exit 4 if they disagree) and against the
//! wall-clock capture window (`length_vs_window`; a warning only, because it
//! includes start-up latency), peak level, clips, gaps and drops, and a final
//! machine-readable line:
//!
//! `SCK_PROBE result=<ok|permission_denied|no_audio|count_mismatch|tone_missing|error|skipped> ...`
//!
//! Exit codes: 0 ok (and "skipped" on non-macOS), 1 error, 2 permission not
//! granted, 3 the stream started but no audio arrived, 4 the sample count does
//! not match the elapsed time, 5 `--tone` was given but the tone did not come
//! back. Nothing here needs `sudo`.

#![cfg_attr(not(target_os = "macos"), allow(dead_code))]

/// Test tone: 997 Hz (a prime, so it never lines up with a block size),
/// -20 dBFS (amplitude 0.1).
const TONE_HZ: f64 = 997.0;
const TONE_AMPLITUDE: f32 = 0.1;

/// The sample count must match the OS clock span to within this fraction.
const COUNT_TOLERANCE: f64 = 0.005;

/// Warn when the recording is shorter than this fraction of the wall-clock
/// capture window (start-up latency alone costs a few percent of 3 s).
const WINDOW_WARN_RATIO: f64 = 0.9;

struct Args {
    seconds: f64,
    out: Option<std::path::PathBuf>,
    keep: bool,
    request: bool,
    tone: bool,
    report: Option<std::path::PathBuf>,
}

fn parse_args() -> Result<Args, String> {
    let mut a = Args {
        seconds: 3.0,
        out: None,
        keep: false,
        request: false,
        tone: false,
        report: None,
    };
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--seconds" => {
                a.seconds = it
                    .next()
                    .and_then(|v| v.parse().ok())
                    .filter(|&s: &f64| s > 0.0 && s <= 3600.0)
                    .ok_or("--seconds needs a number between 0 and 3600")?
            }
            "--out" => a.out = Some(it.next().ok_or("--out needs a path")?.into()),
            "--report" => a.report = Some(it.next().ok_or("--report needs a path")?.into()),
            "--keep" => a.keep = true,
            "--request" => a.request = true,
            "--tone" => a.tone = true,
            "-h" | "--help" => return Err("usage: sck_probe [--seconds N] [--out FILE] [--keep] [--request] [--tone] [--report FILE]".into()),
            other => return Err(format!("unknown argument {other:?} (try --help)")),
        }
    }
    Ok(a)
}

/// Frequency of the strongest periodic signal on one channel, from
/// interpolated positive-going zero crossings over the part of the signal
/// louder than -40 dBFS. Returns `None` if there is no such part.
fn estimate_frequency(samples: &[f32], channels: usize, channel: usize, rate: u32) -> Option<f64> {
    let x: Vec<f32> = samples
        .iter()
        .skip(channel)
        .step_by(channels)
        .copied()
        .collect();
    let loud = |s: &f32| s.abs() > 0.01;
    let first = x.iter().position(loud)?;
    let last = x.iter().rposition(loud)?;
    let mut crossings = Vec::new();
    for i in first..last {
        let (a, b) = (x[i], x[i + 1]);
        if a < 0.0 && b >= 0.0 {
            crossings.push(i as f64 + f64::from(-a / (b - a)));
        }
    }
    if crossings.len() < 10 {
        return None;
    }
    let span = crossings[crossings.len() - 1] - crossings[0];
    Some((crossings.len() - 1) as f64 / span * f64::from(rate))
}

/// Write the test tone to `path` as float WAV (using the crate's own writer).
fn write_tone(path: &std::path::Path, rate: u32, seconds: f64) -> std::io::Result<()> {
    let mut w = jk_recorder::wav::WavWriter::create(path, jk_recorder::wav::WavSpec::new(rate, 2))?;
    let frames = (seconds * f64::from(rate)) as usize;
    let mut buf = Vec::with_capacity(frames * 2);
    for i in 0..frames {
        let s = (TONE_AMPLITUDE as f64
            * (2.0 * std::f64::consts::PI * TONE_HZ * i as f64 / f64::from(rate)).sin())
            as f32;
        buf.push(s);
        buf.push(s);
    }
    w.write_interleaved(&buf)?;
    w.finalize().map(|_| ())
}

fn main() {
    let code = match parse_args() {
        Ok(args) => run(args),
        Err(msg) => {
            eprintln!("{msg}");
            println!("SCK_PROBE result=error reason=\"bad arguments\"");
            1
        }
    };
    std::process::exit(code);
}

#[cfg(not(target_os = "macos"))]
fn run(_args: Args) -> i32 {
    println!(
        "sck_probe tests macOS ScreenCaptureKit capture; nothing to do on {}.",
        std::env::consts::OS
    );
    println!("SCK_PROBE result=skipped reason=\"macOS only\"");
    0
}

#[cfg(target_os = "macos")]
fn run(args: Args) -> i32 {
    use jk_recorder::{PermissionState, Recorder, RecorderConfig, RecorderError, to_dbfs};
    use std::fmt::Write as _;
    use std::time::{Duration, Instant};

    let mut report = String::new();
    macro_rules! say {
        ($($t:tt)*) => {{
            let line = format!($($t)*);
            println!("{line}");
            let _ = writeln!(report, "{line}");
        }};
    }
    let finish = |report: &str, result: &str, extra: &str, code: i32| -> i32 {
        let line = format!("SCK_PROBE result={result} {extra}");
        println!("{line}");
        if let Some(p) = &args.report
            && let Err(e) = std::fs::write(p, format!("{report}{line}\n"))
        {
            eprintln!("could not write report {}: {e}", p.display());
        }
        code
    };

    say!("jk-recorder sck_probe {}", env!("CARGO_PKG_VERSION"));
    say!("os: {}", jk_recorder::os_version());
    say!("backend: {}", jk_recorder::backend().name());
    let device = jk_recorder::output_device();
    match &device {
        Some(d) => say!(
            "output device: {} | nominal {} Hz | physical {} bit, {} ch",
            d.name,
            d.nominal_sample_rate,
            d.physical_bits.map_or("?".into(), |b| b.to_string()),
            d.physical_channels.map_or("?".into(), |c| c.to_string())
        ),
        None => say!("output device: none reported by Core Audio"),
    }
    let mut permission = jk_recorder::permission_preflight();
    say!("permission (CGPreflightScreenCaptureAccess): {permission:?}");
    if permission != PermissionState::Granted && args.request {
        permission = jk_recorder::permission_request();
        say!("permission after CGRequestScreenCaptureAccess: {permission:?}");
    }

    let out = args.out.clone().unwrap_or_else(|| {
        std::env::temp_dir().join(format!("sck_probe-{}.wav", std::process::id()))
    });
    let mut config = RecorderConfig::new(&out);
    config.request_permission = args.request;

    let started = Instant::now();
    let recorder = match Recorder::start(config) {
        Ok(r) => r,
        Err(RecorderError::PermissionDenied(why)) => {
            say!("capture: permission denied: {why}");
            return finish(
                &report,
                "permission_denied",
                &format!("permission={permission:?}"),
                2,
            );
        }
        Err(e) => {
            say!("capture: could not start: {e}");
            return finish(&report, "error", &format!("reason=\"{e}\""), 1);
        }
    };
    say!(
        "capture started in {:.0} ms",
        started.elapsed().as_secs_f64() * 1000.0
    );
    // Wall-clock window between "the OS says the stream runs" and stop().
    // The ratio check below compares the recording with the OS timestamps of
    // the buffers that arrived, so it cannot see audio missing BEFORE the
    // first buffer or AFTER the last one (e.g. if the OS sends nothing while
    // the system is silent). This window is the independent yardstick.
    let window_start = Instant::now();
    for n in &recorder.status().setup.notes {
        say!("note: {n}");
    }

    // Optional test tone through the default output, played by a separate
    // process so it is part of the system mix.
    let mut player = None;
    let tone_path = std::env::temp_dir().join(format!("sck_probe-tone-{}.wav", std::process::id()));
    if args.tone {
        let rate = device
            .as_ref()
            .map(|d| d.nominal_sample_rate.round() as u32)
            .filter(|&r| r > 0)
            .unwrap_or(48_000);
        let _ = std::fs::remove_file(&tone_path);
        match write_tone(&tone_path, rate, args.seconds + 1.0) {
            Ok(()) => match std::process::Command::new("/usr/bin/afplay")
                .arg(&tone_path)
                .spawn()
            {
                Ok(child) => player = Some(child),
                Err(e) => say!("tone: could not start afplay: {e}"),
            },
            Err(e) => say!("tone: could not write the tone file: {e}"),
        }
    }

    let deadline = Instant::now() + Duration::from_secs_f64(args.seconds);
    while Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(500));
        let st = recorder.status();
        say!(
            "  t={:.1}s state={:?} frames={} peak dBFS={}",
            st.stats.wall_elapsed.as_secs_f64(),
            st.state,
            st.stats.frames,
            st.levels
                .peak_dbfs()
                .iter()
                .map(|d| format!("{d:.1}"))
                .collect::<Vec<_>>()
                .join("/")
        );
    }
    if let Some(mut p) = player.take() {
        let _ = p.kill();
        let _ = p.wait();
    }
    let _ = std::fs::remove_file(&tone_path);

    let window_s = window_start.elapsed().as_secs_f64();
    let done = match recorder.stop() {
        Ok(d) => d,
        Err(e) => {
            say!("capture: could not stop cleanly: {e}");
            return finish(&report, "error", &format!("reason=\"{e}\""), 1);
        }
    };
    for line in done.report().lines() {
        say!("{line}");
    }

    let s = &done.stats;
    let Some(format) = s.format.clone() else {
        say!(
            "verdict: the stream started but no audio buffer arrived in {:.1} s",
            args.seconds
        );
        return finish(
            &report,
            "no_audio",
            &format!("permission={permission:?}"),
            3,
        );
    };
    let ratio = s.frames_per_os_second_ratio();
    // Recording length against the wall-clock capture window. The recording
    // spans Recorder::start() to stop() (silence before the first and after
    // the last buffer is filled), so this should be close to 1.0; the window
    // starts after start() returned, so it is slightly shorter than Start..Stop.
    let length_vs_window = if window_s > 0.0 {
        s.media_elapsed_secs / window_s
    } else {
        0.0
    };
    say!(
        "recording length {:.3} s in a capture window of {window_s:.3} s (ratio {length_vs_window:.4})",
        s.media_elapsed_secs
    );
    if length_vs_window < WINDOW_WARN_RATIO {
        // Reported, not (yet) a failure: this path has never run on a Mac.
        say!(
            "warning: the recording is {:.3} s shorter than the capture window; the OS may deliver \
             no audio during silence before the first or after the last buffer (that time is not \
             filled with silence)",
            window_s - s.media_elapsed_secs
        );
    }
    let peak = done
        .levels
        .peak_total
        .iter()
        .copied()
        .fold(0.0f32, f32::max);
    let mut tone_hz = None;
    if args.tone
        && done.file.is_some()
        && let Ok((_, data)) = jk_recorder::wav::read_wav_f32(&out)
    {
        tone_hz = estimate_frequency(&data, usize::from(format.channels), 0, format.sample_rate);
    }
    let extra = format!(
        "permission={permission:?} rate={} channels={} source=\"{}\" frames={} os_span_s={} ratio={} \
         window_s={window_s:.3} length_vs_window={length_vs_window:.4} peak_dbfs={:.2} \
         clipped={} gaps={} gap_frames={} dropped={} tone_hz={} head={} tail={} \
         length_vs_start_stop={} edge_refused={}",
        format.sample_rate,
        format.channels,
        format.source,
        s.frames,
        s.os_clock_elapsed_secs
            .map_or("n/a".into(), |t| format!("{t:.6}")),
        ratio.map_or("n/a".into(), |r| format!("{r:.6}")),
        to_dbfs(peak),
        s.clipped_samples,
        s.gap_events,
        s.gap_frames,
        s.dropped_frames,
        tone_hz.map_or("n/a".into(), |f| format!("{f:.2}")),
        s.head_fill_frames,
        s.tail_fill_frames,
        s.length_vs_start_stop_ratio()
            .map_or("n/a".into(), |r| format!("{r:.6}")),
        s.edge_fill_refused
    );
    if !args.keep {
        let _ = std::fs::remove_file(&out);
    } else {
        say!("capture kept at {}", out.display());
    }
    if let Some(e) = &s.error {
        say!("verdict: capture error: {e}");
        return finish(&report, "error", &extra, 1);
    }
    if let Some(r) = ratio
        && (r - 1.0).abs() > COUNT_TOLERANCE
    {
        say!("verdict: sample count does not match the OS clock (ratio {r:.6})");
        return finish(&report, "count_mismatch", &extra, 4);
    }
    if args.tone {
        let ok = tone_hz.is_some_and(|f| (f - TONE_HZ).abs() < 2.0);
        if !ok {
            say!("verdict: the {TONE_HZ} Hz test tone did not come back (measured {tone_hz:?})");
            return finish(&report, "tone_missing", &extra, 5);
        }
    }
    say!("verdict: ok");
    finish(&report, "ok", &extra, 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frequency_estimate_is_accurate() {
        let rate = 48_000;
        let n = rate as usize;
        let mut x = vec![0.0f32; n * 2];
        for i in 0..n {
            let s = 0.1 * (2.0 * std::f64::consts::PI * TONE_HZ * i as f64 / f64::from(rate)).sin();
            x[2 * i] = s as f32;
            x[2 * i + 1] = s as f32;
        }
        let f = estimate_frequency(&x, 2, 0, rate).unwrap();
        assert!((f - TONE_HZ).abs() < 0.01, "{f}");
        assert!(estimate_frequency(&vec![0.0; 1000], 2, 0, rate).is_none());
    }
}

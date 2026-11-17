// SPDX-License-Identifier: GPL-3.0-or-later
//! The self-check (WORK_ORDER 4.10, docs/APP_API.md section 10).
//!
//! The only tests that will ever run on the owner's MacBook are the ones the owner
//! starts, so this writes a plain-text report into the Settings folder on the stick
//! (`selfcheck-<local time>.txt`) for the next session to read, and every failure
//! says what to do about it.
//!
//! Items, in order:
//! 1. System: OS version (macOS: at least 13.0), app version and layout.
//! 2. Web engine: navigator.userAgent from the UI; WebView2 runtime version
//!    (Windows) or WebKit.framework's CFBundleVersion (macOS).
//! 3. Memory: total, available, this process's resident size.
//! 4. Output device: name and format the engine opened.
//! 5. Test tone: a generated song with a 440 Hz tone is played for 1 s through the
//!    real output device, either at -60 dBFS peak or as digital silence (master volume
//!    0; `--selfcheck --silent`, used on the build PC), and the playback clock must
//!    advance in step with real time with no underruns.
//! 6. Recorder: permission state, then a 3-second capture while the test song keeps
//!    playing (Windows loopback delivers nothing when nothing plays).
//! 7. Demo song: the bundled demo song opens and renders offline (same DSP as
//!    playback), with the right length and a finite, non-silent output.
//! 8. Key change: the test tone rendered at +2 and -5 semitones; the measured pitch
//!    must be within 5 cents of the target and the length unchanged.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};

use serde::Serialize;

use crate::core::Core;
use crate::engine::{ExportFormat, MixParams, PlaybackState};

#[derive(Clone, Debug, Serialize)]
pub struct Item {
    pub name: String,
    pub ok: bool,
    pub detail: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct SelfcheckResult {
    pub report_path: String,
    pub passed: bool,
    pub items: Vec<Item>,
}

#[derive(Clone, Debug, Default)]
pub struct Options {
    /// navigator.userAgent, as the UI reported it.
    pub user_agent: Option<String>,
    /// Digital silence instead of the -60 dBFS tone.
    pub silent: bool,
}

/// Peak of the audible test tone: -60 dBFS (WORK_ORDER: people are around).
pub const TONE_PEAK: f64 = 0.001;
/// Amplitude stored in the generated test song (rendered at full level offline for
/// the pitch measurement, played through the device scaled down to [`TONE_PEAK`]).
const SONG_AMPLITUDE: f64 = 0.25;

fn item(name: &str, ok: bool, detail: impl Into<String>) -> Item {
    Item {
        name: name.to_string(),
        ok,
        detail: detail.into(),
    }
}

fn mb(b: Option<u64>) -> String {
    b.map_or("unknown".into(), |b| format!("{} MB", b / (1024 * 1024)))
}

/// WebKit.framework's CFBundleVersion (macOS) - the real WebKit build, whatever
/// Safari version is installed.
#[cfg(target_os = "macos")]
pub fn webkit_version() -> Option<String> {
    use objc2_foundation::{NSBundle, NSString};
    let bundle = NSBundle::bundleWithIdentifier(&NSString::from_str("com.apple.WebKit"))?;
    let info = bundle.infoDictionary()?;
    let read = |key: &str| -> Option<String> {
        let v = info.objectForKey(&NSString::from_str(key))?;
        let s = v.downcast::<NSString>().ok()?;
        Some(s.to_string())
    };
    let version = read("CFBundleVersion")?;
    Some(match read("CFBundleShortVersionString") {
        Some(short) => format!("{short} (CFBundleVersion {version})"),
        None => format!("CFBundleVersion {version}"),
    })
}

#[cfg(not(target_os = "macos"))]
pub fn webkit_version() -> Option<String> {
    None
}

/// Runs everything and writes the report. Blocking (about 10-30 s).
pub fn run(core: &Core, opts: &Options) -> SelfcheckResult {
    let started = Instant::now();
    let mut items = Vec::new();
    let tmp = core.paths.tmp.join("selfcheck");
    let _ = std::fs::create_dir_all(&tmp);

    items.push(check_system(core));
    items.push(check_web_engine(opts));
    items.push(check_memory());

    // Generate the test song (also used by the key-change check).
    let tone_path = tmp.join("Jukeoke - Test Tone.jksong");
    let tone = crate::testsong::write(
        &tone_path,
        crate::testsong::ToneSong {
            amplitude: SONG_AMPLITUDE,
            ..Default::default()
        },
    );
    if let Err(e) = &tone {
        items.push(item(
            "Test song",
            false,
            format!("The test song could not be written to {}: {e}. Check that the stick is not full or write-protected.", tmp.display()),
        ));
    }

    let (device_item, tone_item, recorder_item) = if tone.is_ok() {
        play_and_capture(core, &tone_path, opts.silent)
    } else {
        (
            check_output_device(core),
            item("Test tone", false, "Skipped: no test song."),
            item("Recorder", false, "Skipped: no test song to record."),
        )
    };
    items.push(device_item);
    items.push(tone_item);
    items.push(recorder_item);

    items.push(check_demo_song(core, &tmp));
    if tone.is_ok() {
        items.push(check_key_change(core, &tone_path, &tmp));
    }
    let _ = std::fs::remove_file(&tone_path);

    let passed = items.iter().all(|i| i.ok);
    let report_path = write_report(core, opts, &items, passed, started.elapsed());
    SelfcheckResult {
        report_path: report_path.display().to_string(),
        passed,
        items,
    }
}

fn check_system(core: &Core) -> Item {
    let os = crate::sysinfo::os_version();
    let mut ok = true;
    let mut detail = format!(
        "{os}; {} {} ({} {}); data folder: {} ({:?}: {})",
        core.name,
        core.version,
        std::env::consts::OS,
        std::env::consts::ARCH,
        core.layout.data_root.display(),
        core.layout.mode,
        core.layout.reason
    );
    if cfg!(target_os = "macos") {
        let major = os
            .strip_prefix("macOS ")
            .and_then(|v| v.split('.').next())
            .and_then(|m| m.parse::<u32>().ok());
        if major.is_some_and(|m| m < 13) {
            ok = false;
            detail.push_str(". Jukeoke needs macOS 13 (Ventura) or newer.");
        }
    }
    if let Some(v) = &core.webview_env_ignored {
        let _ = write!(detail, "; WEBVIEW2_USER_DATA_FOLDER={v} was ignored (portable profile kept on the stick)");
    }
    item("System", ok, detail)
}

fn check_web_engine(opts: &Options) -> Item {
    let ua = opts.user_agent.clone();
    let engine = if cfg!(target_os = "macos") {
        webkit_version().map(|v| format!("WebKit.framework {v}"))
    } else {
        tauri::webview_version().ok().map(|v| format!("WebView2 runtime {v}"))
    };
    let detail = format!(
        "{}; navigator.userAgent: {}",
        engine.clone().unwrap_or_else(|| "web engine version unknown".into()),
        ua.clone().unwrap_or_else(|| "not reported (run without the window)".into())
    );
    // Passing needs the engine version; the user agent is informational (headless runs
    // have none).
    item("Web engine", engine.is_some(), detail)
}

fn check_memory() -> Item {
    let m = crate::sysinfo::memory();
    let mut detail = format!(
        "total {}, available {}, this process {}",
        mb(m.total_bytes),
        mb(m.available_bytes),
        mb(m.process_rss_bytes)
    );
    let low = m.available_bytes.is_some_and(|a| a < 512 * 1024 * 1024);
    if low {
        detail.push_str(". Less than 512 MB is free: close other apps (browsers especially) before singing, or playback may stutter.");
    }
    item("Memory", m.total_bytes.is_some(), detail)
}

fn device_text(d: &crate::engine::DeviceStatus) -> String {
    format!(
        "{}: {} Hz, {} channels, {} mode, output latency {:.1} ms",
        d.name,
        d.sample_rate,
        d.channels,
        if d.exclusive { "exclusive" } else { "shared" },
        d.latency_ms
    )
}

fn check_output_device(core: &Core) -> Item {
    match core.player.engine().snapshot().device {
        Some(d) => item("Output device", true, device_text(&d)),
        None => match core.player.engine().list_devices() {
            Ok(list) if !list.is_empty() => item(
                "Output device",
                true,
                list.iter()
                    .map(|d| format!("{}{}: {} Hz, {} ch", d.name, if d.is_default { " (default)" } else { "" }, d.sample_rate, d.channels))
                    .collect::<Vec<_>>()
                    .join("; "),
            ),
            Ok(_) => item("Output device", false, "No audio output device was found. Connect headphones or speakers (or turn the built-in speakers on) and run the self-check again."),
            Err(e) => item("Output device", false, format!("The output devices could not be listed: {}", e.message)),
        },
    }
}

/// Plays the test song through the device (tone or silence) and records 3 s of what
/// the computer plays meanwhile. Restores the user's song and volume afterwards.
fn play_and_capture(core: &Core, tone_path: &Path, silent: bool) -> (Item, Item, Item) {
    let player = &core.player;
    let engine = player.engine();
    // Remember what the user had, to put it back.
    let previous = player.current();
    let prev_snap = engine.snapshot();
    let master = core.settings.get().audio.master_volume as f32;
    player.saver.flush();
    // While the test song plays, the app shows no song (the UI must not think the
    // user's song is playing a 440 Hz tone).
    player.set_current(None);

    let gain = if silent { 0.0 } else { (TONE_PEAK / SONG_AMPLITUDE) as f32 };
    // The engine smooths master volume changes per device buffer. Give the ramp from
    // the user's volume down to the test level time to finish on silence (nothing is
    // loaded yet), so not even the first milliseconds of the tone exceed -60 dBFS.
    engine.unload();
    engine.set_master(gain);
    std::thread::sleep(Duration::from_millis(250));
    let load = engine.load(tone_path, MixParams::default());
    let mut device_item = check_output_device(core);
    let tone_item;
    let recorder_item;
    match load {
        Err(e) => {
            tone_item = item("Test tone", false, format!("The test song could not be loaded: {} ({})", e.message, e.code));
            recorder_item = item("Recorder", false, "Skipped: the test song could not be played.");
        }
        Ok(_) => {
            engine.play();
            // Measure the clock rate only once sound is leaving the device. The start
            // (render-ahead priming plus the device's output latency, which is a few
            // hundred ms on Bluetooth) is not a rate error, and counting it made the
            // ratio 0.89 on the null device, close to the 0.85 limit (verifier,
            // 2026-11-17). Wait up to 3 s for the clock to move.
            let play_called = Instant::now();
            let first = loop {
                let s = engine.snapshot();
                let stopped = matches!(s.state, PlaybackState::Error | PlaybackState::Ended);
                if s.position_s > 0.0 || stopped || play_called.elapsed() > Duration::from_secs(3) {
                    break s;
                }
                std::thread::sleep(Duration::from_millis(5));
            };
            let start_delay = play_called.elapsed().as_secs_f64();
            let t0 = Instant::now();
            let p0 = first.position_s;
            std::thread::sleep(Duration::from_millis(1200));
            let snap = engine.snapshot();
            // Time between the two device callbacks that produced the positions (exact),
            // or the wall time between the two reads when the engine gave no callback time.
            let wall = if first.position_ns != 0 && snap.position_ns > first.position_ns {
                (snap.position_ns - first.position_ns) as f64 * 1e-9
            } else {
                t0.elapsed().as_secs_f64()
            };
            let advanced = snap.position_s - p0;
            if let Some(d) = &snap.device {
                device_item = item("Output device", true, device_text(d));
            }
            let what = if silent {
                "digital silence (master volume 0) instead of a tone, because this run was started with --silent; this checks that the device opens and consumes audio in real time"
                    .to_string()
            } else {
                "a 440 Hz tone at -60 dBFS peak (quiet on purpose; it may be barely audible)".to_string()
            };
            let peak = snap.output_peak[0].max(snap.output_peak[1]) as f64;
            let ratio = if wall > 0.0 { advanced / wall } else { 0.0 };
            let level_ok = if silent { peak == 0.0 } else { peak <= TONE_PEAK * 1.01 };
            let ok = snap.state == PlaybackState::Playing
                && (0.85..=1.15).contains(&ratio)
                && snap.underruns == 0
                && level_ok;
            let started = if first.position_s > 0.0 {
                format!("Sound reached the device {:.0} ms after play.", start_delay * 1000.0)
            } else {
                format!("The playback clock had not moved {start_delay:.1} s after play.")
            };
            let mut detail = format!(
                "Played {what}. {started} The playback clock then advanced {advanced:.3} s in {wall:.3} s of real time (ratio {ratio:.3}); underruns {}; device starvation events {}; output peak {:.1} dBFS.",
                snap.underruns,
                snap.device_starved,
                if peak > 0.0 { 20.0 * peak.log10() } else { f64::NEG_INFINITY }
            );
            if !ok {
                if snap.state != PlaybackState::Playing {
                    let _ = write!(detail, " Playback is not running (state {:?}{}). Check that the output device is connected and not used exclusively by another app.", snap.state, snap.error.as_deref().map(|e| format!(": {e}")).unwrap_or_default());
                } else if !(0.85..=1.15).contains(&ratio) {
                    detail.push_str(" The device is not consuming audio at its rate: try another output device, or restart the computer's audio.");
                } else if snap.underruns > 0 {
                    detail.push_str(" Audio dropped out: close other heavy apps and run the self-check again; if it repeats, note the number for the next session.");
                } else if !level_ok {
                    detail.push_str(" The output level was not what was asked for: report this (it suggests processing between the app and the device).");
                }
            }
            tone_item = item("Test tone", ok, detail);
            recorder_item = check_recorder(core);
            engine.pause();
        }
    }
    engine.unload();
    engine.set_master(master);
    // Put the user's song back, paused where it was.
    if let Some(prev) = previous {
        match engine.load(&prev.path, MixParams::from_settings(&prev.settings)) {
            Ok(_) => {
                engine.seek(prev_snap.position_s);
                player.set_current(Some(prev));
            }
            Err(e) => crate::applog::warn(&format!("self-check: could not reload {}: {e}", prev.song_id)),
        }
    }
    (device_item, tone_item, recorder_item)
}

fn check_recorder(core: &Core) -> Item {
    let permission = core.recorder.permission();
    if cfg!(target_os = "macos") && permission != "granted" {
        return item(
            "Recorder",
            false,
            format!(
                "Screen Recording permission: {permission}. macOS calls the system-audio permission \"Screen Recording\". \
                 Open System Settings > Privacy & Security > Screen Recording, switch Jukeoke on, quit and reopen Jukeoke, \
                 then run the self-check again. (If Jukeoke is already on that list but this still fails, the app was rebuilt: \
                 switch it off and on again.)"
            ),
        );
    }
    let path = core.paths.tmp.join("selfcheck").join(format!("capture-{:08x}.wav", crate::fs_atomic::random_u32()));
    // The recording covers the time from the start() call (the backend's set-up time,
    // about 0.5 s on Windows and possibly longer for ScreenCaptureKit, is filled with
    // silence), so its expected length is the start-to-stop time, not the 3 s sleep.
    let call = Instant::now();
    let started = jk_recorder::Recorder::start(jk_recorder::RecorderConfig::new(&path));
    let rec = match started {
        Ok(r) => r,
        Err(e) => {
            let (_, msg) = crate::recorder::user_message(&e);
            return item("Recorder", false, format!("Permission: {permission}. The capture could not start: {msg} (technical: {e})"));
        }
    };
    let setup_s = call.elapsed().as_secs_f64();
    std::thread::sleep(Duration::from_secs(3));
    let finished = rec.stop();
    let call_to_stop = call.elapsed().as_secs_f64();
    let _ = std::fs::remove_file(&path);
    match finished {
        Err(e) => item("Recorder", false, format!("The capture failed while stopping: {e}")),
        Ok(f) => {
            let s = &f.stats;
            let report = f.report().replace('\n', "; ");
            let len = s.media_elapsed_secs;
            // WORK_ORDER section 9: the sample count must match the elapsed time. The
            // recorder's own yardstick is the OS-clock time from its start to its stop
            // instant; without one, the length must lie between the 3 s of capture and
            // the time from the start() call to the end of stop().
            let ratio = s.length_vs_start_stop_ratio();
            let length_ok = len >= 2.9
                && len <= call_to_stop + 0.05
                && ratio.is_none_or(|r| (0.98..=1.02).contains(&r));
            let ok = f.file.is_some() && length_ok && s.dropped_frames == 0 && s.error.is_none();
            let mut detail = format!(
                "Permission: {permission}. The capture started in {setup_s:.3} s and recorded {len:.3} s ({}) for 3 s of capture. {report}",
                ratio.map_or("no OS start/stop times".to_string(), |r| format!("length / start-to-stop time {r:.5}"))
            );
            if f.file.is_none() {
                detail.push_str(" No audio arrived from the system during the capture. On a Mac, check the Screen Recording permission; on Windows, check that the default output device is the one Jukeoke plays to.");
            } else if s.dropped_frames > 0 {
                detail.push_str(" Some audio was lost because the disk could not keep up: record to a faster drive.");
            } else if !length_ok {
                detail.push_str(" The recording's length does not match the time it ran: recordings from this computer may drift against real time. Please bring this report back.");
            }
            item("Recorder", ok, detail)
        }
    }
}

/// Where the bundled demo song is.
pub fn find_demo_song(core: &Core) -> Option<PathBuf> {
    core.demo_song.clone().filter(|p| p.is_file())
}

fn check_demo_song(core: &Core, tmp: &Path) -> Item {
    let Some(demo) = find_demo_song(core) else {
        return item(
            "Demo song",
            false,
            "The bundled demo song (resources/demo-song.jksong) is missing from this copy of the app. Copy the app folder again from a complete build.",
        );
    };
    let song = match jk_format::SongFile::open(&demo) {
        Ok(s) => s,
        Err(e) => return item("Demo song", false, format!("The demo song at {} cannot be opened: {}", demo.display(), e.user_message())),
    };
    let frames = song.manifest().duration_frames;
    let rate = song.manifest().sample_rate;
    drop(song);
    let out = tmp.join("demo-render.wav");
    let t = Instant::now();
    let r = core.renderer.render(&demo, &MixParams::default(), &out, ExportFormat::Wav32f, &mut |_| {}, &AtomicBool::new(false));
    let secs = t.elapsed().as_secs_f64();
    let _ = std::fs::remove_file(&out);
    match r {
        Err(e) => item("Demo song", false, format!("Rendering the demo song failed: {} ({}; {})", e.message, e.code, e.detail)),
        Ok(rep) => {
            let ok = rep.frames == frames && rep.peak.is_finite() && rep.peak > 0.0 && rep.sample_rate == rate;
            let mut d = format!(
                "Rendered {:.2} s of audio ({} frames at {} Hz) in {secs:.2} s ({:.1}x real time); peak {:.4}",
                frames as f64 / rate.max(1) as f64,
                rep.frames,
                rep.sample_rate,
                (frames as f64 / rate.max(1) as f64) / secs.max(1e-6),
                rep.peak
            );
            if !ok {
                let _ = write!(d, ". Expected {frames} frames at {rate} Hz and a non-silent, finite output: the render is wrong; please report it.");
            }
            item("Demo song", ok, d)
        }
    }
}

/// Reads a 32-bit float WAV written by the engine (first channel only).
fn read_f32_wav_ch0(path: &Path) -> Option<(u32, Vec<f32>)> {
    let h = crate::recorder::read_wav_header(path)?;
    if h.bits != 32 {
        return None;
    }
    let bytes = std::fs::read(path).ok()?;
    let data_start = bytes.len() as u64 - h.data_bytes;
    let ch = h.channels as usize;
    let frames = h.data_bytes as usize / (4 * ch);
    let mut out = Vec::with_capacity(frames);
    for i in 0..frames {
        let o = data_start as usize + i * 4 * ch;
        out.push(f32::from_le_bytes(bytes[o..o + 4].try_into().ok()?));
    }
    Some((h.sample_rate, out))
}

fn check_key_change(core: &Core, tone_path: &Path, tmp: &Path) -> Item {
    let song_frames = jk_format::SongFile::open(tone_path).map(|s| s.manifest().duration_frames).unwrap_or(0);
    let mut parts = Vec::new();
    let mut ok = true;
    for semitones in [2i32, -5] {
        let out = tmp.join(format!("key{semitones:+}.wav"));
        let p = MixParams {
            semitones,
            ..MixParams::default()
        };
        let r = core.renderer.render(tone_path, &p, &out, ExportFormat::Wav32f, &mut |_| {}, &AtomicBool::new(false));
        let measured = match r {
            Err(e) => {
                ok = false;
                parts.push(format!("{semitones:+}: render failed: {} ({})", e.message, e.code));
                let _ = std::fs::remove_file(&out);
                continue;
            }
            Ok(rep) => {
                let wav = read_f32_wav_ch0(&out);
                let _ = std::fs::remove_file(&out);
                (rep, wav)
            }
        };
        let (rep, Some((rate, x))) = measured else {
            ok = false;
            parts.push(format!("{semitones:+}: the rendered file could not be read back"));
            continue;
        };
        // Measure in the middle 2 s (away from the start-up of the pitch shifter).
        let mid = x.len() / 2;
        let half = (rate as usize).min(mid);
        let seg = &x[mid - half..mid + half];
        let target = 440.0 * 2f64.powf(semitones as f64 / 12.0);
        let f = jk_dsp::analysis::estimate_frequency(seg, rate, target, 100.0);
        let cents = jk_dsp::analysis::cents(f, target);
        let length_ok = rep.frames == song_frames;
        let pitch_ok = cents.abs() <= 5.0;
        ok &= length_ok && pitch_ok;
        parts.push(format!(
            "{semitones:+} semitones: target {target:.3} Hz, measured {f:.3} Hz ({cents:+.3} cents), length {} of {song_frames} frames{}",
            rep.frames,
            if length_ok && pitch_ok { "" } else { " - WRONG" }
        ));
    }
    let mut detail = parts.join("; ");
    if !ok {
        detail.push_str(". The key change does not give the right pitch or length on this computer: please bring this report back.");
    }
    item("Key change", ok, detail)
}

fn write_report(core: &Core, opts: &Options, items: &[Item], passed: bool, took: Duration) -> PathBuf {
    let stamp = crate::sysinfo::local_stamp().replace(' ', "_");
    let path = core.paths.settings.join(format!("selfcheck-{stamp}.txt"));
    let mut t = String::new();
    let _ = writeln!(t, "{} self-check", core.name);
    let _ = writeln!(t, "==============================");
    let _ = writeln!(t, "result: {}", if passed { "PASSED" } else { "FAILED" });
    let _ = writeln!(t, "time (UTC): {}", jk_format::time::utc_now_iso8601());
    let _ = writeln!(t, "local time: {}", crate::sysinfo::local_stamp());
    let _ = writeln!(t, "app: {} {} ({})", core.name, core.version, core.identifier);
    let _ = writeln!(t, "mode: {}", if opts.silent { "silent (digital silence instead of the test tone)" } else { "test tone at -60 dBFS" });
    let _ = writeln!(t, "took: {:.1} s", took.as_secs_f64());
    let _ = writeln!(t);
    for i in items {
        let _ = writeln!(t, "[{}] {}", if i.ok { " OK " } else { "FAIL" }, i.name);
        let _ = writeln!(t, "    {}", i.detail);
        let _ = writeln!(t);
    }
    let _ = writeln!(t, "Bring this file back to the Windows PC (it is in the Settings folder on the stick).");
    let text = t.replace('\n', "\r\n");
    if let Err(e) = crate::fs_atomic::write_atomic(&path, text.as_bytes()) {
        crate::applog::error(&format!("self-check report could not be written to {}: {e}", path.display()));
    }
    crate::applog::info(&format!("self-check {}: {}", if passed { "passed" } else { "failed" }, path.display()));
    path
}

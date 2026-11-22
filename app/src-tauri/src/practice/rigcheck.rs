// SPDX-License-Identifier: GPL-3.0-or-later
//! The practice rig check (WO2 4.8; docs/APP_API.md 10.2): the one check that MAKES A
//! SOUND. The owner starts it by hand from Settings with the mixer plugged in and the
//! in-ears on; the call is refused unless `audible_confirmed` is true. It is never part of
//! the self-check, `--selfcheck` or any automated test (tests drive this code on the null
//! output device with a generated input, so nothing plays aloud).
//!
//! Steps (`practice://rigcheck { step, progress, message }`): `silence` (2 s: the noise
//! floor per channel), `voice` (3 s: "say a few words": which channel carries the voice),
//! `test_sound` (three 0.5 s bursts of a swept tone with noise, peaking at -24 dBFS, through
//! the current output at the app's master volume), `analysing`, `done`. The report goes to
//! `<root>/Settings/selfcheck-rig-<local time>.txt` (a second run in the same app session
//! is appended, so "loopback on" and "loopback off" sit side by side). A measured delay of
//! the returned test sound is saved for the singer's timing (`practice.measured_latency_ms`,
//! APP_API 12.7). No sound is ever written: the input is measured in memory and dropped.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use jk_recorder::input::InputState;
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::PracticeService;
use super::align::clock_offset_ns;
use super::mic;
use crate::engine::MixParams;
use crate::error::{ApiError, ApiResult};
use crate::events::emit;

/// `practice://rigcheck`.
pub const RIGCHECK_EVENT: &str = "practice://rigcheck";
/// Peak of the test sound (APP_API 10.2: -24 dBFS before the master volume).
pub const TEST_PEAK: f64 = 0.063;
const BURSTS: usize = 3;
const BURST_S: f64 = 0.5;
const GAP_S: f64 = 0.5;
const LEAD_S: f64 = 0.5;
/// Longest return delay looked for (Bluetooth returns arrive 100-300+ ms late).
const MAX_DELAY_S: f64 = 0.6;
/// Normalised correlation above which the test sound "came back".
const DETECT_CORR: f64 = 0.25;

/// The report file of this app session (a second run appends to it).
static REPORT: Mutex<Option<PathBuf>> = Mutex::new(None);

/// `RigCheckOptions`.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct RigCheckOptions {
    #[serde(default)]
    pub audible_confirmed: bool,
    #[serde(default)]
    pub output_device_id: Option<String>,
    #[serde(default)]
    pub input_device_id: Option<String>,
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub voice_step: Option<bool>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct RigOutput {
    pub name: String,
    pub kind: String,
    pub sample_rate: u32,
    pub channels: u16,
    pub format: String,
    pub latency_ms: Option<f64>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct RigInput {
    pub name: String,
    pub kind: String,
    pub sample_rate: u32,
    pub channels: u16,
    pub format: String,
    pub declared_latency_ms: Option<f64>,
    pub same_device_as_output: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct RigChannel {
    pub index: usize,
    pub noise_floor_db: f64,
    pub voice_rms_db: Option<f64>,
    pub test_rms_db: Option<f64>,
    pub carries: &'static str,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct RigReturned {
    pub detected: bool,
    pub channel: Option<usize>,
    pub level_db: Option<f64>,
    pub delay_ms: Option<f64>,
    pub delay_spread_ms: Option<f64>,
}

/// `RigCheckReport`.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct RigCheckReport {
    pub report_path: String,
    pub ok: bool,
    pub label: Option<String>,
    pub output: RigOutput,
    pub input: RigInput,
    pub channels: Vec<RigChannel>,
    pub returned: RigReturned,
    pub findings: Vec<String>,
}

fn db(x: f64) -> f64 {
    if x > 1e-12 { ((20.0 * x.log10()) * 10.0).round() / 10.0 } else { -120.0 }
}

fn rms(x: &[f32]) -> f64 {
    if x.is_empty() {
        return 0.0;
    }
    (x.iter().map(|&v| f64::from(v) * f64::from(v)).sum::<f64>() / x.len() as f64).sqrt()
}

/// The test sound at `rate`: `LEAD_S` of silence, then three bursts of a logarithmic sweep
/// from 300 Hz to 3 kHz with a little noise, each `BURST_S` long with 10 ms fades,
/// `GAP_S` apart, peaking at [`TEST_PEAK`].
pub fn test_sound(rate: u32) -> Vec<f32> {
    let fs = f64::from(rate);
    let total = ((LEAD_S + BURSTS as f64 * (BURST_S + GAP_S) + 0.5) * fs) as usize;
    let mut out = vec![0.0f32; total];
    let mut seed: u32 = 0x1234_5678;
    for b in 0..BURSTS {
        let start = ((LEAD_S + b as f64 * (BURST_S + GAP_S)) * fs) as usize;
        let n = (BURST_S * fs) as usize;
        let (f0, f1) = (300.0f64, 3000.0f64);
        let mut phase = 0.0f64;
        for i in 0..n {
            let t = i as f64 / fs;
            let f = f0 * (f1 / f0).powf(t / BURST_S);
            phase += 2.0 * std::f64::consts::PI * f / fs;
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let noise = (f64::from(seed >> 8) / f64::from(1u32 << 24)) * 2.0 - 1.0;
            let fade = (t / 0.01).min((BURST_S - t) / 0.01).clamp(0.0, 1.0);
            let v = (0.8 * phase.sin() + 0.2 * noise) * fade * TEST_PEAK;
            out[start + i] = v as f32;
        }
    }
    out
}

/// Burst `b`'s start (s) in the test sound.
fn burst_start(b: usize) -> f64 {
    LEAD_S + b as f64 * (BURST_S + GAP_S)
}

/// Writes the test sound as a song (it plays through the normal engine path, so the
/// output tap carries exactly what was played).
fn write_test_song(path: &Path, rate: u32) -> jk_format::Result<()> {
    let x = test_sound(rate);
    let frames = x.len();
    let scale = 8_388_608.0f64;
    let mut other = Vec::with_capacity(frames * 2);
    for &v in &x {
        let s = (f64::from(v) * scale).round().clamp(-scale, scale - 1.0) as i32;
        other.push(s);
        other.push(s);
    }
    let silence = vec![0i32; frames * 2];
    let roles = ["lead_vocal", "backing_vocals", "drums", "other"];
    let stems: Vec<serde_json::Value> = roles
        .iter()
        .map(|r| {
            json!({"role": r, "chunk": format!("stem.{r}"), "codec": "flac", "bits": 24, "gain": 1.0,
                   "peak": if *r == "other" { TEST_PEAK } else { 0.0 },
                   "in_karaoke_mix": *r != "lead_vocal", "pitched": *r != "drums"})
        })
        .collect();
    let manifest: jk_format::Manifest = serde_json::from_value(json!({
        "format": "jksong", "format_version": jk_format::FORMAT_VERSION,
        "title": "Rig check test sound", "artist": "Open Mic", "album": null,
        "sample_rate": rate, "channels": 2, "duration_frames": frames,
        "duration_seconds": frames as f64 / f64::from(rate),
        "cover": null, "lyrics": null, "analysis": null,
        "preset": "full", "stems_sum_to_source": false, "stems": stems,
        "provenance": {"app": {"name": "Open Mic rig check", "version": env!("CARGO_PKG_VERSION")},
                       "created_utc": jk_format::time::utc_now_iso8601(), "resampling": [], "models": [],
                       "source": {"filename": "generated", "lossy": false}}
    }))
    .map_err(|e| jk_format::FormatError::InvalidInput(e.to_string()))?;
    let mut w = jk_format::SongWriter::new(manifest);
    for r in roles {
        let samples = if r == "other" { &other } else { &silence };
        let flac = jk_engine::encode_flac(samples, 2, 24, rate);
        w.add_chunk(format!("stem.{r}"), "audio/flac", jk_format::ChunkData::Bytes(flac))?;
    }
    w.write_atomic(path)?;
    Ok(())
}

/// What was captured while the test sound played: the reference (mono, at the tap rate,
/// with the host time of each piece) and the input (all channels, with block stamps).
#[derive(Default)]
pub struct Capture {
    pub ref_rate: u32,
    /// (host ns of the first frame, mono frames).
    pub reference: Vec<(i64, Vec<f32>)>,
    pub in_rate: u32,
    pub channels: usize,
    /// (host ns of the first frame captured, interleaved frames).
    pub input: Vec<(i64, Vec<f32>)>,
    /// The input latency the device declares (s), taken off the input stamps.
    pub declared_s: f64,
}

/// A series of contiguous pieces (each with the host time of its first frame) as one
/// signal: the samples end to end, and a least-squares line host = a + b * index through
/// the pieces' stamps (device threads wake up with a few milliseconds of jitter, so single
/// stamps are not exact; the line through all of them is).
fn series(pieces: &[(i64, Vec<f32>)], stride: usize, ch: usize, rate: u32) -> (Vec<f32>, f64, f64) {
    let mut x = Vec::new();
    let mut pts = Vec::with_capacity(pieces.len());
    for (t, v) in pieces {
        pts.push((x.len() as f64, *t as f64));
        x.extend(v.chunks_exact(stride.max(1)).map(|f| f[ch]));
    }
    let nominal = 1e9 / f64::from(rate.max(1));
    if pts.len() < 3 {
        let t0 = pts.first().map_or(0.0, |p| p.1);
        return (x, t0, nominal);
    }
    let m = pts.len() as f64;
    let (mi, mt) = (pts.iter().map(|p| p.0).sum::<f64>() / m, pts.iter().map(|p| p.1).sum::<f64>() / m);
    let (mut sii, mut sit) = (0.0, 0.0);
    for &(i, t) in &pts {
        sii += (i - mi) * (i - mi);
        sit += (i - mi) * (t - mt);
    }
    let b = if sii > 0.0 { sit / sii } else { nominal };
    let b = if (b / nominal - 1.0).abs() < 0.01 { b } else { nominal };
    (x, mt - b * mi, b)
}

/// Per burst: the delay (s) beyond the stamps, the level (dB re what was played) and the
/// normalised correlation, for one input channel; `None` when the burst is not there.
pub fn find_return(c: &Capture, ch: usize, song_t0_host: i64) -> Vec<Option<(f64, f64, f64)>> {
    let fs = f64::from(c.in_rate.max(1));
    let (refs, ra, rb) = series(&c.reference, 1, 0, c.ref_rate);
    let (inp, ia, ib) = series(&c.input, c.channels, ch, c.in_rate);
    if refs.is_empty() || inp.is_empty() {
        return vec![None; BURSTS];
    }
    let declared = c.declared_s * 1e9;
    // The reference at host time t (linear interpolation; it is resampled to the input
    // rate this way when the two rates differ).
    let ref_at = |t: f64| -> f32 {
        let x = (t - ra) / rb;
        if x < 0.0 {
            return 0.0;
        }
        let k = x.floor() as usize;
        let f = (x - k as f64) as f32;
        match (refs.get(k), refs.get(k + 1)) {
            (Some(a), Some(b)) => a + (b - a) * f,
            (Some(a), None) => *a,
            _ => 0.0,
        }
    };
    let max_lag = (MAX_DELAY_S * fs) as usize;
    (0..BURSTS)
        .map(|b| {
            let n = (BURST_S * fs) as usize;
            let start_host = song_t0_host as f64 + burst_start(b) * 1e9;
            let r: Vec<f32> = (0..n).map(|j| ref_at(start_host + j as f64 * 1e9 / fs)).collect();
            let rr = r.iter().map(|&v| f64::from(v) * f64::from(v)).sum::<f64>();
            if rr <= 1e-12 {
                return None;
            }
            // The input frame captured (declared latency off) when the burst left the DAC.
            let j0 = (start_host + declared - ia) / ib;
            if j0 < 0.0 {
                return None;
            }
            let j0 = j0.round() as usize;
            // Normalised correlation of the burst with the input `lag` frames after j0.
            let corr = |lag: usize| -> Option<(f64, f64)> {
                let a = j0 + lag;
                if a + n > inp.len() {
                    return None;
                }
                let seg = &inp[a..a + n];
                let xy: f64 = seg.iter().zip(&r).map(|(&x, &y)| f64::from(x) * f64::from(y)).sum();
                let yy: f64 = seg.iter().map(|&x| f64::from(x) * f64::from(x)).sum();
                Some((if yy > 0.0 { xy / (rr * yy).sqrt() } else { 0.0 }, xy / rr))
            };
            // Coarse search on 4x box-averaged series (a sixteenth of the work), then every
            // lag around the coarse peak at the full rate.
            const D: usize = 4;
            let dec = |v: &[f32], from: usize, len: usize| -> Vec<f32> {
                (0..len / D).map(|k| v[from + k * D..from + k * D + D].iter().sum::<f32>()).collect()
            };
            let rd = dec(&r, 0, n);
            let rrd: f64 = rd.iter().map(|&v| f64::from(v) * f64::from(v)).sum();
            let mut coarse = (0usize, f64::MIN);
            let mut lag = 0usize;
            while lag < max_lag && j0 + lag + n <= inp.len() {
                let seg = dec(&inp, j0 + lag, n);
                let xy: f64 = seg.iter().zip(&rd).map(|(&x, &y)| f64::from(x) * f64::from(y)).sum();
                let yy: f64 = seg.iter().map(|&x| f64::from(x) * f64::from(x)).sum();
                let c = if yy > 0.0 && rrd > 0.0 { xy / (rrd * yy).sqrt() } else { 0.0 };
                if c > coarse.1 {
                    coarse = (lag, c);
                }
                lag += D;
            }
            let mut best = (0usize, 0.0f64, 0.0f64);
            for lag in coarse.0.saturating_sub(2 * D)..(coarse.0 + 2 * D).min(max_lag) {
                if let Some((c, g)) = corr(lag)
                    && c > best.1
                {
                    best = (lag, c, g);
                }
            }
            (best.1 >= DETECT_CORR).then(|| (best.0 as f64 / fs, db(best.2.abs()), best.1))
        })
        .collect()
}

impl PracticeService {
    /// `practice_rig_check(options)`. See the module documentation. Blocking (about 10 s).
    pub fn rig_check(&self, opts: &RigCheckOptions) -> ApiResult<RigCheckReport> {
        if !opts.audible_confirmed {
            return Err(ApiError::new(
                "audible_not_confirmed",
                "The rig check plays a test sound. Put your in-ear monitors on, then confirm to start it.",
            ));
        }
        if self.take_running() {
            return Err(ApiError::new("practising", "Finish or stop the practice take first; the rig check plays its own sound."));
        }
        if self.rig_running.swap(true, Ordering::SeqCst) {
            return Err(ApiError::new("busy", "The rig check is already running."));
        }
        let r = self.rig_check_inner(opts);
        self.rig_running.store(false, Ordering::SeqCst);
        r
    }

    fn rig_step(&self, step: &str, progress: f64, message: &str) {
        emit(&**self.sink(), RIGCHECK_EVENT, &json!({"step": step, "progress": progress, "message": message}));
    }

    fn rig_check_inner(&self, opts: &RigCheckOptions) -> ApiResult<RigCheckReport> {
        let player = self.player();
        let engine = player.engine();
        // Playback paused and practice input suspended first (APP_API 10.2).
        let _ = player.pause();
        self.suspend_input(true)?;
        let spec = self.rig_input(opts.input_device_id.as_deref()).map_err(|m| ApiError::new("no_input", m))?;
        let list = if spec.is_test() { Vec::new() } else { jk_recorder::input::list_input_devices().unwrap_or_default() };
        let snap0 = engine.snapshot();
        let out_dev = snap0.device.clone();
        let (mut stream, view) = mic::open_source(&spec, &list, out_dev.as_ref().map(|d| d.name.as_str()), engine).map_err(|e| {
            let _ = self.suspend_input(false);
            ApiError::new("input_unavailable", mic::plain_input_error(&e))
        })?;
        let fmt = stream.format().clone();
        let channels = usize::from(fmt.channels.max(1));
        let mut buf = Vec::new();
        // Per-channel RMS of one step.
        let mut measure = |stream: &mut jk_recorder::input::InputStream, secs: f64, step: &str, msg: &str, p0: f64, p1: f64| -> Vec<f64> {
            let t0 = Instant::now();
            let mut sum = vec![0.0f64; channels];
            let mut n = 0usize;
            let mut last = Instant::now();
            while t0.elapsed().as_secs_f64() < secs {
                while stream.read(&mut buf).is_some() {
                    for f in buf.chunks_exact(channels) {
                        for (c, &v) in f.iter().enumerate() {
                            sum[c] += f64::from(v) * f64::from(v);
                        }
                        n += 1;
                    }
                }
                if last.elapsed() >= Duration::from_millis(250) {
                    last = Instant::now();
                    self.rig_step(step, p0 + (p1 - p0) * (t0.elapsed().as_secs_f64() / secs).min(1.0), msg);
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            buf.iter_mut().for_each(|x| *x = 0.0);
            sum.iter().map(|s| if n > 0 { (s / n as f64).sqrt() } else { 0.0 }).collect()
        };
        let floor = measure(&mut stream, 2.0, "silence", "Measuring the quiet: please stay silent for 2 seconds.", 0.0, 0.2);
        let voice = if opts.voice_step.unwrap_or(true) {
            Some(measure(&mut stream, 3.0, "voice", "Say a few words into your microphone now.", 0.2, 0.45))
        } else {
            None
        };

        // The test sound through the normal engine path; the user's song is put back after.
        self.rig_step("test_sound", 0.45, "Playing the test sound (three short bursts).");
        let rate = out_dev.as_ref().map_or(48_000, |d| d.sample_rate.max(8000));
        let song = self.tmp_dir().join("rigcheck").join("Rig check test sound.jksong");
        let _ = std::fs::create_dir_all(song.parent().unwrap_or(Path::new(".")));
        let previous = player.current();
        player.saver.flush();
        player.set_current(None);
        let restore = |player: &crate::playback::Player| {
            engine.unload();
            if let Some(prev) = previous.clone() {
                match engine.load(&prev.path, MixParams::from_settings(&prev.settings)) {
                    Ok(_) => {
                        engine.seek(snap0.position_s);
                        player.set_current(Some(prev));
                    }
                    Err(e) => crate::applog::warn(&format!("rig check: could not reload {}: {e}", prev.song_id)),
                }
            }
        };
        let mut cap = Capture { in_rate: fmt.sample_rate, channels, declared_s: fmt.declared_latency_ns.unwrap_or(0) as f64 / 1e9, ..Capture::default() };
        let off = clock_offset_ns();
        let played = write_test_song(&song, rate).map_err(|e| ApiError::new("internal", format!("The test sound could not be written: {e}")));
        let mut song_t0_host = None;
        if played.is_ok() {
            engine.unload();
            match engine.load(&song, MixParams::default()) {
                Ok(_) => {
                    let mut tap = engine.output_tap();
                    engine.play();
                    let dur = test_sound(rate).len() as f64 / f64::from(rate) + 0.8;
                    let t0 = Instant::now();
                    let mut tbuf = vec![0.0f32; 2 * 4096];
                    let mut last = Instant::now();
                    while t0.elapsed().as_secs_f64() < dur {
                        if let Some(t) = tap.as_mut() {
                            while let Some(p) = t.read(&mut tbuf) {
                                cap.ref_rate = p.rate;
                                // The song's frame 0 left the DAC at this host time.
                                if song_t0_host.is_none()
                                    && let Some(pos) = p.song_pos
                                {
                                    song_t0_host = Some(p.dac_ns as i64 + off - (pos * 1e9).round() as i64);
                                }
                                cap.reference.push((p.dac_ns as i64 + off, tbuf[..2 * p.frames].chunks_exact(2).map(|f| 0.5 * (f[0] + f[1])).collect()));
                            }
                        }
                        while let Some(st) = stream.read(&mut buf) {
                            cap.input.push((st.host_ns, buf.clone()));
                        }
                        if last.elapsed() >= Duration::from_millis(250) {
                            last = Instant::now();
                            self.rig_step("test_sound", 0.45 + 0.4 * (t0.elapsed().as_secs_f64() / dur).min(1.0), "Playing the test sound.");
                        }
                        std::thread::sleep(Duration::from_millis(5));
                    }
                    engine.pause();
                }
                Err(e) => crate::applog::warn(&format!("rig check: the test sound could not be loaded: {e}")),
            }
        }
        restore(player);
        let input_state = stream.state();
        stream.stop();
        let _ = std::fs::remove_file(&song);
        self.rig_step("analysing", 0.9, "Working out what came back.");

        // Analysis per channel.
        let mut per_ch: Vec<Vec<Option<(f64, f64, f64)>>> = Vec::new();
        for ch in 0..channels {
            per_ch.push(match song_t0_host {
                Some(t0) => find_return(&cap, ch, t0),
                None => vec![None; BURSTS],
            });
        }
        let test_rms: Vec<Option<f64>> = (0..channels)
            .map(|ch| {
                let v: Vec<f32> = cap.input.iter().flat_map(|(_, b)| b.chunks_exact(channels).map(move |f| f[ch])).collect();
                (!v.is_empty()).then(|| rms(&v))
            })
            .collect();
        let mut channels_out = Vec::new();
        for ch in 0..channels {
            let fl = floor.get(ch).copied().unwrap_or(0.0);
            let vo = voice.as_ref().and_then(|v| v.get(ch).copied());
            let has_voice = vo.is_some_and(|v| db(v) > db(fl) + 12.0 && db(v) > -60.0);
            let has_test = per_ch[ch].iter().filter(|x| x.is_some()).count() >= 2;
            channels_out.push(RigChannel {
                index: ch,
                noise_floor_db: db(fl),
                voice_rms_db: vo.map(db),
                test_rms_db: test_rms[ch].map(db),
                carries: match (has_voice, has_test) {
                    (true, true) => "both",
                    (true, false) => "voice",
                    (false, true) => "test_sound",
                    _ => "nothing",
                },
            });
        }
        // The returned test sound: the channel where most bursts came back, best correlation.
        let best_ch = (0..channels)
            .filter(|&c| per_ch[c].iter().filter(|x| x.is_some()).count() >= 2)
            .max_by(|&a, &b| {
                let s = |c: usize| per_ch[c].iter().flatten().map(|x| x.2).sum::<f64>();
                s(a).total_cmp(&s(b))
            });
        if std::env::var_os("JK_RIG_DEBUG").is_some() {
            eprintln!("rig debug: per channel {per_ch:?}; ref pieces {} input blocks {}", cap.reference.len(), cap.input.len());
        }
        let returned = match best_ch {
            Some(c) => {
                let found: Vec<(f64, f64, f64)> = per_ch[c].iter().flatten().copied().collect();
                let delays: Vec<f64> = found.iter().map(|x| x.0 * 1000.0).collect();
                let mean = delays.iter().sum::<f64>() / delays.len() as f64;
                let spread = delays.iter().map(|d| (d - mean).abs()).fold(0.0, f64::max) * 2.0;
                let level = found.iter().map(|x| x.1).sum::<f64>() / found.len() as f64;
                RigReturned {
                    detected: true,
                    channel: Some(c),
                    level_db: Some((level * 10.0).round() / 10.0),
                    delay_ms: Some((mean * 100.0).round() / 100.0),
                    delay_spread_ms: Some((spread * 100.0).round() / 100.0),
                }
            }
            None => RigReturned { detected: false, channel: None, level_db: None, delay_ms: None, delay_spread_ms: None },
        };
        let output = RigOutput {
            name: out_dev.as_ref().map_or_else(|| "none".into(), |d| d.name.clone()),
            kind: if out_dev.as_ref().is_some_and(|d| d.name.to_lowercase().contains("null")) { "null".into() } else { "output".into() },
            sample_rate: out_dev.as_ref().map_or(0, |d| d.sample_rate),
            channels: out_dev.as_ref().map_or(0, |d| d.channels),
            format: "f32 (the engine converts to the device's format)".into(),
            latency_ms: out_dev.as_ref().map(|d| (d.latency_ms * 10.0).round() / 10.0),
        };
        let input = RigInput {
            name: view.name.clone(),
            kind: view.kind.into(),
            sample_rate: fmt.sample_rate,
            channels: fmt.channels,
            format: format!("{} ({})", fmt.sample_format, fmt.source),
            declared_latency_ms: fmt.declared_latency_ns.map(|n| ((n as f64 / 1e6) * 10.0).round() / 10.0),
            same_device_as_output: view.same_device_as_output,
        };
        let mut findings = Vec::new();
        if played.is_err() || song_t0_host.is_none() {
            findings.push("The test sound could not be played, so nothing could be measured about the return. Check the output device.".to_string());
        }
        if let InputState::Failed { message, .. } = &input_state {
            findings.push(format!("The input stopped during the check: {message}"));
        }
        let voice_chs: Vec<usize> = channels_out.iter().filter(|c| matches!(c.carries, "voice" | "both")).map(|c| c.index).collect();
        if voice.is_some() {
            if voice_chs.is_empty() {
                findings.push("No voice was heard on any channel. Check the microphone's gain on the mixer and that the mixer sends to the computer.".into());
            } else {
                findings.push(format!(
                    "Your voice is on channel {} (counting from 1).",
                    voice_chs.iter().map(|c| (c + 1).to_string()).collect::<Vec<_>>().join(" and ")
                ));
            }
        }
        match (&returned.detected, returned.delay_ms, returned.delay_spread_ms) {
            (true, Some(d), Some(s)) => {
                let path = if s > 5.0 { "a wandering delay, as over Bluetooth: practice needs the music on a cable or over USB" } else { "a fixed delay, as over a cable or USB" };
                findings.push(format!(
                    "The mixer sends the app's sound back on channel {}, {:.1} ms late beyond the declared latencies, at {:.1} dB ({path}).",
                    returned.channel.map_or(0, |c| c + 1),
                    d,
                    returned.level_db.unwrap_or(0.0)
                ));
                findings.push("While practising, the app will warn about the music in the input and remove it where it can; to give it a clean voice, switch the mixer's loopback off.".into());
            }
            _ => findings.push("The test sound did not come back through the input: the mixer is not sending the app's music back (the clean setup for practice).".into()),
        }
        // A measured fixed delay becomes the singer's timing correction (APP_API 12.7).
        let settings = self.settings_store();
        let me = crate::keymode::machine_id();
        let out_id = settings.get().audio.device_id.unwrap_or_else(|| "default".into());
        if let (true, Some(d), Some(s)) = (returned.detected, returned.delay_ms, returned.delay_spread_ms)
            && s <= 5.0
        {
            let key = format!("{me}|{}|{out_id}", view.id);
            settings.update(|st| {
                st.practice.measured_latency_ms.insert(
                    key.clone(),
                    crate::settings::MeasuredLatency { ms: d, how: "rig_check".into(), utc: jk_format::time::utc_now_iso8601(), extra: Default::default() },
                );
            });
        }
        let ok = played.is_ok() && song_t0_host.is_some() && !matches!(input_state, InputState::Failed { .. });
        let mut report = RigCheckReport {
            report_path: String::new(),
            ok,
            label: opts.label.clone().filter(|l| !l.trim().is_empty()),
            output,
            input,
            channels: channels_out,
            returned,
            findings,
        };
        report.report_path = self.write_rig_report(&report).display().to_string();
        let _ = self.suspend_input(false);
        self.rig_step("done", 1.0, "Done.");
        Ok(report)
    }

    fn write_rig_report(&self, r: &RigCheckReport) -> PathBuf {
        let mut slot = REPORT.lock().unwrap_or_else(|e| e.into_inner());
        let path = slot.clone().unwrap_or_else(|| {
            let stamp = jk_format::time::utc_now_iso8601().replace([':', '-'], "").replace('T', "-").trim_end_matches('Z').to_string();
            self.settings_dir().join(format!("selfcheck-rig-{stamp}.txt"))
        });
        let mut t = String::new();
        let _ = writeln!(t, "Open Mic practice rig check ({}){}", jk_format::time::utc_now_iso8601(), r.label.as_deref().map(|l| format!(" - {l}")).unwrap_or_default());
        let _ = writeln!(t, "AUDIBLE CHECK: it played three short bursts of a test sound at -24 dBFS (before the master volume).");
        let o = &r.output;
        let _ = writeln!(t, "Output: {} ({}), {} Hz, {} ch, {}, latency {:?} ms", o.name, o.kind, o.sample_rate, o.channels, o.format, o.latency_ms);
        let i = &r.input;
        let _ = writeln!(t, "Input: {} ({}), {} Hz, {} ch, {}, declared latency {:?} ms, same device as output: {}", i.name, i.kind, i.sample_rate, i.channels, i.format, i.declared_latency_ms, i.same_device_as_output);
        for c in &r.channels {
            let _ = writeln!(t, "  channel {}: noise floor {:.1} dBFS, voice {:?} dBFS, during the test sound {:?} dBFS, carries {}", c.index + 1, c.noise_floor_db, c.voice_rms_db, c.test_rms_db, c.carries);
        }
        let x = &r.returned;
        let _ = writeln!(t, "Test sound came back: {} (channel {:?}, level {:?} dB, delay {:?} ms beyond the declared latencies, spread {:?} ms over the bursts)", x.detected, x.channel.map(|c| c + 1), x.level_db, x.delay_ms, x.delay_spread_ms);
        for f in &r.findings {
            let _ = writeln!(t, "- {f}");
        }
        let _ = writeln!(t, "No sound was recorded or saved.\n");
        use std::io::Write;
        let res = std::fs::OpenOptions::new().create(true).append(true).open(&path).and_then(|mut f| f.write_all(t.as_bytes()));
        if let Err(e) = res {
            crate::applog::warn(&format!("rig check: report not written ({}): {e}", path.display()));
        }
        *slot = Some(path.clone());
        path
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_test_sound_peaks_at_minus_24_dbfs() {
        let x = test_sound(48_000);
        let peak = x.iter().fold(0.0f32, |a, &v| a.max(v.abs()));
        assert!((f64::from(peak) - TEST_PEAK).abs() < 0.003, "peak {peak}");
        assert!(20.0 * f64::from(peak).log10() <= -23.9);
    }

    #[test]
    fn a_returned_test_sound_is_found_with_its_delay_and_level() {
        // Reference: the test sound leaving the DAC from host time 1 s; input: the same
        // returned 23 ms later at -12 dB on channel 1 of 2 (channel 0 silent noise).
        let rate = 48_000;
        let x = test_sound(rate);
        let t0: i64 = 1_000_000_000;
        let mut c = Capture { ref_rate: rate, in_rate: rate, channels: 2, ..Default::default() };
        for (k, piece) in x.chunks(480).enumerate() {
            c.reference.push((t0 + (k as f64 * 480.0 * 1e9 / 48_000.0) as i64, piece.to_vec()));
        }
        let delay = (0.023 * 48_000.0) as usize;
        let g = 10f32.powf(-12.0 / 20.0);
        let total = x.len() + 48_000;
        let mut inter = vec![0.0f32; total * 2];
        for j in 0..total {
            let src = j as i64 - delay as i64;
            inter[2 * j + 1] = if src >= 0 && (src as usize) < x.len() { x[src as usize] * g } else { 0.0 };
            inter[2 * j] = 1e-5 * ((j * 7919 % 13) as f32 - 6.0);
        }
        for (k, piece) in inter.chunks(960).enumerate() {
            c.input.push((t0 + (k as f64 * 480.0 * 1e9 / 48_000.0) as i64, piece.to_vec()));
        }
        let found = find_return(&c, 1, t0);
        assert!(found.iter().all(Option::is_some), "{found:?}");
        for f in found.iter().flatten() {
            assert!((f.0 * 1000.0 - 23.0).abs() < 0.05, "delay {} ms", f.0 * 1000.0);
            assert!((f.1 + 12.0).abs() < 0.5, "level {} dB", f.1);
        }
        assert!(find_return(&c, 0, t0).iter().all(Option::is_none));
    }
}

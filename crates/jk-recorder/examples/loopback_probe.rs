// SPDX-License-Identifier: GPL-3.0-or-later
//! `loopback_probe`: the Windows recorder self-check and benchmark
//! (WORK_ORDER sections 4.6 and 9, trap 5; results in docs/benchmarks/recorder.md).
//!
//! ```text
//! cargo run --release -p jk-recorder --example loopback_probe -- [options]
//!
//!   --list            list the output devices and exit
//!   --device ID       record (and play the test signal on) this output device
//!                     instead of the default one
//!   --seconds N       recording length (default 20)
//!   --silence N       seconds of total silence in the middle (default 8)
//!   --no-keepalive    switch off the recorder's silent keep-alive stream, to
//!                     show that silent stretches are filled from the timestamps
//!   --silence-only    play nothing at all: start and stop in silence
//!   --wait-quiet N    wait up to N s for other apps' audio sessions on the
//!                     device to go inactive (exit 7 "busy" if they do not)
//!   --grab-exclusive  device-lost test: after 2 s, open the device in exclusive
//!                     mode (digital zeros, 1 s), which invalidates the loopback
//!                     stream; checks the recording stops visibly (exit 8 if not)
//!   --alloc-check     count heap allocations in the whole process during 5 s of
//!                     steady recording (nothing else runs; should be 0)
//!   --out FILE.wav    keep the capture here (default: a temp file, deleted)
//!   --report FILE     also write the plain-text report to FILE
//! ```
//!
//! With the default schedule the probe starts recording in silence, plays
//! episode A (pseudo-random sequence 1 s, 997 Hz sine 2.5 s, pseudo-random
//! sequence 1 s), stops its player completely for `--silence` seconds (total
//! silence: no stream of this probe plays anything except the recorder's own
//! keep-alive zeros), plays episode B (1 s, 2 s, 1 s), and stops recording in
//! silence. The test signal peaks at 2^-11 (-66.2 dBFS), leaving room for
//! Windows' endpoint effects (they raised a 2^-10 signal to -58.0 dBFS on the
//! build PC); if the captured output still exceeds -60 dBFS after episode A,
//! the probe stops (`result=too_loud`, exit 1). It is written to the device in
//! its own float mix format, so it reaches the mix without any conversion.
//!
//! Then it checks, from the file alone:
//!
//! * the captured sample rate equals the device mix rate;
//! * the sample count against the OS-clock time from Start to Stop (ratio and
//!   frames; it must agree within one 10 ms packet) and against the wall clock;
//! * no dropouts: each episode is aligned on its first pseudo-random segment,
//!   and its last segment must still sit at exactly the same offset;
//! * bit-transparency: max |captured - played| over every sample of both
//!   episodes, the least-squares gain, and the level of anything else in the
//!   capture (other apps, device effects);
//! * the 997 Hz sine's frequency and level.
//!
//! Last line: `LOOPBACK_PROBE result=<ok|...> key=value ...`. Exit codes:
//! 0 ok, 1 error, 3 sample rate differs from the device, 4 length differs from
//! Start..Stop by more than one packet, 5 the test signal was not found,
//! 6 dropout or sample slip, 7 other apps kept playing (`--wait-quiet`) or
//! played during the test (the length check is still valid then; the signal
//! checks are not), 8 `--grab-exclusive`: the device loss was not reported.
//! A capture that is aligned and complete but not bit-identical exits 0 and
//! says `bit_exact=no` (that is Windows' processing, reported, not a recorder
//! failure).

#![cfg_attr(not(windows), allow(dead_code))]

/// Counts every heap allocation in this process (for `--alloc-check`).
mod alloc_count {
    use std::alloc::{GlobalAlloc, Layout, System};
    use std::sync::atomic::{AtomicU64, Ordering};

    pub static ALLOCS: AtomicU64 = AtomicU64::new(0);

    pub struct Counting;

    // SAFETY: forwards every call to the system allocator unchanged.
    unsafe impl GlobalAlloc for Counting {
        unsafe fn alloc(&self, l: Layout) -> *mut u8 {
            ALLOCS.fetch_add(1, Ordering::Relaxed);
            unsafe { System.alloc(l) }
        }
        unsafe fn alloc_zeroed(&self, l: Layout) -> *mut u8 {
            ALLOCS.fetch_add(1, Ordering::Relaxed);
            unsafe { System.alloc_zeroed(l) }
        }
        unsafe fn realloc(&self, p: *mut u8, l: Layout, n: usize) -> *mut u8 {
            ALLOCS.fetch_add(1, Ordering::Relaxed);
            unsafe { System.realloc(p, l, n) }
        }
        unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
            unsafe { System.dealloc(p, l) }
        }
    }
}

#[global_allocator]
static GLOBAL: alloc_count::Counting = alloc_count::Counting;

/// Test-signal amplitude: 2^-11 = -66.23 dBFS peak (exactly representable).
/// The hard rule is "nothing aloud above -60 dBFS", and the loopback (which
/// taps the mix after Windows' endpoint effects, i.e. what the DAC receives)
/// showed that this PC's enhancements raise the peak: at 2^-10 (-60.2 dBFS)
/// the captured output peaked at -58.0 dBFS. 2^-11 leaves 6 dB for that, and
/// `LOUDEST_ALLOWED` below stops the test if the output still exceeds -60 dBFS.
const AMP: f32 = 1.0 / 2048.0;
/// -60 dBFS: the loudest the probe may make the device play, measured on the
/// loopback capture itself (after any endpoint effects).
const LOUDEST_ALLOWED: f32 = 1.0e-3;
const TONE_HZ: f64 = 997.0;
/// Episode layouts in seconds: (prbs, sine, prbs).
const EPISODE_A: (f64, f64, f64) = (1.0, 2.5, 1.0);
const EPISODE_B: (f64, f64, f64) = (1.0, 2.0, 1.0);
/// Silence before episode A (the recording starts in silence).
const LEAD_IN_S: f64 = 1.0;
/// At least this much silence after episode B (it stops in silence).
const MIN_TAIL_S: f64 = 1.0;

struct Args {
    list: bool,
    device: Option<String>,
    seconds: f64,
    silence: f64,
    keepalive: bool,
    silence_only: bool,
    wait_quiet: f64,
    grab_exclusive: bool,
    alloc_check: bool,
    out: Option<std::path::PathBuf>,
    report: Option<std::path::PathBuf>,
}

fn parse_args() -> Result<Args, String> {
    let mut a = Args {
        list: false,
        device: None,
        seconds: 20.0,
        silence: 8.0,
        keepalive: true,
        silence_only: false,
        wait_quiet: 0.0,
        grab_exclusive: false,
        alloc_check: false,
        out: None,
        report: None,
    };
    let mut it = std::env::args().skip(1);
    let num = |v: Option<String>, what: &str| -> Result<f64, String> {
        v.and_then(|v| v.parse().ok())
            .filter(|&s: &f64| s > 0.0 && s <= 3600.0)
            .ok_or(format!(
                "{what} needs a number of seconds between 0 and 3600"
            ))
    };
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--list" => a.list = true,
            "--device" => a.device = Some(it.next().ok_or("--device needs an id")?),
            "--seconds" => a.seconds = num(it.next(), "--seconds")?,
            "--silence" => a.silence = num(it.next(), "--silence")?,
            "--no-keepalive" => a.keepalive = false,
            "--silence-only" => a.silence_only = true,
            "--wait-quiet" => a.wait_quiet = num(it.next(), "--wait-quiet")?,
            "--grab-exclusive" => a.grab_exclusive = true,
            "--alloc-check" => a.alloc_check = true,
            "--out" => a.out = Some(it.next().ok_or("--out needs a path")?.into()),
            "--report" => a.report = Some(it.next().ok_or("--report needs a path")?.into()),
            "-h" | "--help" => {
                return Err(
                    "usage: loopback_probe [--list] [--device ID] [--seconds N] [--silence N] \
                            [--no-keepalive] [--silence-only] [--wait-quiet N] [--out FILE.wav] [--report FILE]"
                        .into(),
                );
            }
            other => return Err(format!("unknown argument {other:?} (try --help)")),
        }
    }
    let needed =
        LEAD_IN_S + episode_len(EPISODE_A) + a.silence + episode_len(EPISODE_B) + MIN_TAIL_S;
    if !a.silence_only && a.seconds < needed {
        return Err(format!(
            "--seconds {} is too short for the schedule; it needs at least {needed} s with --silence {}",
            a.seconds, a.silence
        ));
    }
    Ok(a)
}

fn episode_len(e: (f64, f64, f64)) -> f64 {
    e.0 + e.1 + e.2
}

/// xorshift32: a small, well-known pseudo-random generator (deterministic).
struct XorShift(u32);
impl XorShift {
    fn next(&mut self) -> u32 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.0 = x;
        x
    }
}

/// One episode of the test signal, interleaved, with the sample index of each
/// segment boundary: [prbs][sine][prbs]. Each channel gets its own sequence.
struct Episode {
    samples: Vec<f32>,
    frames: usize,
    /// Frame ranges of the three segments.
    seg: [(usize, usize); 3],
}

fn make_episode(layout: (f64, f64, f64), rate: u32, channels: usize, seed: u32) -> Episode {
    let f = |s: f64| (s * f64::from(rate)).round() as usize;
    let (n1, n2, n3) = (f(layout.0), f(layout.1), f(layout.2));
    let frames = n1 + n2 + n3;
    let mut samples = vec![0.0f32; frames * channels];
    for c in 0..channels {
        let mut r1 = XorShift(
            seed.wrapping_mul(2_654_435_761)
                .wrapping_add(c as u32 * 97 + 1)
                | 1,
        );
        let mut r3 = XorShift(seed.wrapping_mul(40_503).wrapping_add(c as u32 * 131 + 7) | 1);
        for i in 0..frames {
            let v = if i < n1 {
                if r1.next() & 1 == 0 { AMP } else { -AMP }
            } else if i < n1 + n2 {
                let k = (i - n1) as f64;
                (f64::from(AMP)
                    * (2.0 * std::f64::consts::PI * TONE_HZ * k / f64::from(rate)).sin())
                    as f32
            } else if r3.next() & 1 == 0 {
                AMP
            } else {
                -AMP
            };
            samples[i * channels + c] = v;
        }
    }
    Episode {
        samples,
        frames,
        seg: [(0, n1), (n1, n1 + n2), (n1 + n2, frames)],
    }
}

/// Frequency from interpolated positive-going zero crossings of one channel.
fn estimate_frequency(x: &[f32], rate: u32) -> Option<f64> {
    let mut crossings = Vec::new();
    for i in 0..x.len().saturating_sub(1) {
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

/// Correlation of the template channel 0 with the capture channel 0 at `pos`.
fn corr_at(cap: &[f32], channels: usize, pos: usize, tpl: &[f32]) -> f64 {
    let mut s = 0.0f64;
    for (i, &t) in tpl.iter().enumerate() {
        let j = (pos + i) * channels;
        if j >= cap.len() {
            break;
        }
        s += f64::from(cap[j]) * f64::from(t);
    }
    s
}

/// Best alignment of `tpl` (channel 0 of a segment) near `guess` (+-`radius`).
fn align(cap: &[f32], channels: usize, guess: usize, radius: usize, tpl: &[f32]) -> (usize, f64) {
    let lo = guess.saturating_sub(radius);
    let hi = guess + radius;
    let energy: f64 = tpl.iter().map(|&t| f64::from(t) * f64::from(t)).sum();
    let mut best = (lo, f64::MIN);
    for pos in lo..=hi {
        let c = corr_at(cap, channels, pos, tpl);
        if c > best.1 {
            best = (pos, c);
        }
    }
    (best.0, best.1 / energy.max(f64::MIN_POSITIVE))
}

fn main() {
    let code = match parse_args() {
        Ok(args) => run(args),
        Err(msg) => {
            eprintln!("{msg}");
            println!("LOOPBACK_PROBE result=error reason=\"bad arguments\"");
            1
        }
    };
    std::process::exit(code);
}

#[cfg(not(windows))]
fn run(_args: Args) -> i32 {
    println!(
        "loopback_probe tests Windows WASAPI loopback capture; nothing to do on {} (see sck_probe on macOS).",
        std::env::consts::OS
    );
    println!("LOOPBACK_PROBE result=skipped reason=\"Windows only\"");
    0
}

#[cfg(windows)]
mod win {
    //! The probe's own WASAPI pieces: a player for the test signal and a few
    //! read-only device facts. Nothing here changes any Windows setting.

    use std::time::Duration;
    use wasapi::{DeviceEnumerator, Direction, SampleType, StreamMode};
    use windows::Win32::Media::Audio::Endpoints::IAudioEndpointVolume;
    use windows::Win32::System::Com::CLSCTX_ALL;

    pub fn device(id: Option<&str>) -> Result<wasapi::Device, String> {
        let en = DeviceEnumerator::new().map_err(|e| e.to_string())?;
        match id {
            Some(id) => en.get_device(id).map_err(|e| e.to_string()),
            None => en
                .get_default_device(Direction::Render)
                .map_err(|e| e.to_string()),
        }
    }

    /// Read-only facts about the endpoint for the report.
    pub fn facts(id: Option<&str>) -> Vec<String> {
        let mut out = Vec::new();
        let Ok(d) = device(id) else {
            return vec!["device: could not open".into()];
        };
        if let Ok(c) = d.get_iaudioclient()
            && let Ok(m) = c.get_mixformat()
        {
            out.push(format!(
                "mix format: {} Hz, {} ch, {} bit {:?}, channel mask 0x{:X}",
                m.get_samplespersec(),
                m.get_nchannels(),
                m.get_bitspersample(),
                m.get_subformat().ok(),
                m.get_dwchannelmask()
            ));
        }
        if let Ok(f) = d.get_device_format() {
            out.push(format!(
                "hardware (engine device) format: {} Hz, {} ch, {} valid bits",
                f.get_samplespersec(),
                f.get_nchannels(),
                f.get_validbitspersample()
            ));
        }
        // SAFETY: activating a documented interface on a valid IMMDevice; read-only calls.
        let vol: Result<IAudioEndpointVolume, _> =
            unsafe { d.as_immdevice().Activate(CLSCTX_ALL, None) };
        match vol {
            Ok(v) => unsafe {
                let scalar = v.GetMasterVolumeLevelScalar().unwrap_or(f32::NAN);
                let db = v.GetMasterVolumeLevel().unwrap_or(f32::NAN);
                let mute = v.GetMute().map(|b| b.as_bool()).unwrap_or(false);
                let hw = v.QueryHardwareSupport().unwrap_or(0);
                out.push(format!(
                    "endpoint volume (read only, not changed): {:.0} % ({db:.2} dB), mute {mute}, \
                     hardware volume control {}",
                    scalar * 100.0,
                    if hw & 1 != 0 { "yes" } else { "no" }
                ));
            },
            Err(e) => out.push(format!("endpoint volume: not readable ({e})")),
        }
        out
    }

    /// Other audio sessions currently active on the endpoint (other apps
    /// that may be playing into the mix during the test).
    pub fn active_sessions(id: Option<&str>) -> Vec<String> {
        let mut out = Vec::new();
        let me = std::process::id();
        let Ok(d) = device(id) else { return out };
        let Ok(mgr) = d.get_iaudiosessionmanager() else {
            return out;
        };
        let Ok(en) = mgr.get_audiosessionenumerator() else {
            return out;
        };
        let n = en.get_count().unwrap_or(0);
        for i in 0..n {
            let Ok(s) = en.get_session(i) else { continue };
            if !matches!(s.get_state(), Ok(wasapi::SessionState::Active)) {
                continue;
            }
            let pid = s.get_process_id().unwrap_or(0);
            if pid == me {
                continue;
            }
            out.push(format!("{} (pid {pid})", process_name(pid)));
        }
        out
    }

    /// The executable name of a process (read only), for the report.
    fn process_name(pid: u32) -> String {
        use windows::Win32::Foundation::CloseHandle;
        use windows::Win32::System::Threading::{
            OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
            QueryFullProcessImageNameW,
        };
        // SAFETY: query-only access; the handle is closed below; the buffer
        // length is passed in.
        unsafe {
            let Ok(h) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else {
                return "unknown".into();
            };
            let mut buf = [0u16; 512];
            let mut len = buf.len() as u32;
            let ok = QueryFullProcessImageNameW(
                h,
                PROCESS_NAME_WIN32,
                windows::core::PWSTR(buf.as_mut_ptr()),
                &mut len,
            )
            .is_ok();
            let _ = CloseHandle(h);
            if !ok {
                return "unknown".into();
            }
            let path = String::from_utf16_lossy(&buf[..len as usize]);
            path.rsplit(['\\', '/'])
                .next()
                .unwrap_or("unknown")
                .to_string()
        }
    }

    /// Names of the Windows audio effect types (AUDIO_EFFECT_TYPE_*).
    fn effect_name(id: windows::core::GUID) -> String {
        use windows::Win32::Media::KernelStreaming::*;
        let known = [
            (
                AUDIO_EFFECT_TYPE_ACOUSTIC_ECHO_CANCELLATION,
                "echo cancellation",
            ),
            (AUDIO_EFFECT_TYPE_NOISE_SUPPRESSION, "noise suppression"),
            (
                AUDIO_EFFECT_TYPE_AUTOMATIC_GAIN_CONTROL,
                "automatic gain control",
            ),
            (AUDIO_EFFECT_TYPE_BEAMFORMING, "beamforming"),
            (
                AUDIO_EFFECT_TYPE_CONSTANT_TONE_REMOVAL,
                "constant tone removal",
            ),
            (AUDIO_EFFECT_TYPE_EQUALIZER, "equalizer"),
            (AUDIO_EFFECT_TYPE_LOUDNESS_EQUALIZER, "loudness equalizer"),
            (AUDIO_EFFECT_TYPE_BASS_BOOST, "bass boost"),
            (AUDIO_EFFECT_TYPE_VIRTUAL_SURROUND, "virtual surround"),
            (AUDIO_EFFECT_TYPE_VIRTUAL_HEADPHONES, "virtual headphones"),
            (AUDIO_EFFECT_TYPE_SPEAKER_FILL, "speaker fill"),
            (AUDIO_EFFECT_TYPE_ROOM_CORRECTION, "room correction"),
            (AUDIO_EFFECT_TYPE_BASS_MANAGEMENT, "bass management"),
            (
                AUDIO_EFFECT_TYPE_ENVIRONMENTAL_EFFECTS,
                "environmental effects",
            ),
            (AUDIO_EFFECT_TYPE_SPEAKER_PROTECTION, "speaker protection"),
            (
                AUDIO_EFFECT_TYPE_SPEAKER_COMPENSATION,
                "speaker compensation",
            ),
            (
                AUDIO_EFFECT_TYPE_DYNAMIC_RANGE_COMPRESSION,
                "dynamic range compression",
            ),
        ];
        known
            .iter()
            .find(|(g, _)| *g == id)
            .map_or_else(|| format!("{id:?}"), |(_, n)| (*n).to_string())
    }

    /// Open the endpoint in exclusive mode and play digital silence (zeros)
    /// for `secs`. Windows then invalidates every shared-mode stream on the
    /// device, the recorder's loopback included: a real "device lost" without
    /// unplugging anything or changing any setting.
    pub fn grab_exclusive(id: Option<&str>, secs: f64) -> Result<String, String> {
        use wasapi::WaveFormat;
        let d = device(id)?;
        let mut client = d.get_iaudioclient().map_err(|e| e.to_string())?;
        let mix = client.get_mixformat().map_err(|e| e.to_string())?;
        let (period, _) = client.get_device_period().map_err(|e| e.to_string())?;
        let (rate, ch) = (
            mix.get_samplespersec() as usize,
            mix.get_nchannels() as usize,
        );
        let mut found = None;
        for (bits, valid, st) in [
            (32, 32, SampleType::Float),
            (32, 24, SampleType::Int),
            (24, 24, SampleType::Int),
            (16, 16, SampleType::Int),
        ] {
            let want = WaveFormat::new(bits, valid, st, rate, ch, None);
            if let Ok(ok) = client.is_supported_exclusive_with_quirks(&want) {
                found = Some(ok);
                break;
            }
        }
        let fmt = found.ok_or("the device accepts no exclusive-mode format")?;
        let period = client
            .calculate_aligned_period_near(period, Some(128), &fmt)
            .unwrap_or(period);
        client
            .initialize_client(
                &fmt,
                Direction::Render,
                StreamMode::EventsExclusive { period_hns: period },
                None,
            )
            .map_err(|e| format!("exclusive open refused: {e}"))?;
        let event = client.set_get_eventhandle().map_err(|e| e.to_string())?;
        let render = client.get_audiorenderclient().map_err(|e| e.to_string())?;
        let frames = client.get_buffer_size().map_err(|e| e.to_string())? as usize;
        let zeros = vec![0u8; frames * fmt.get_blockalign() as usize];
        render
            .write_to_device(&zeros, None)
            .map_err(|e| e.to_string())?;
        client.start_stream().map_err(|e| e.to_string())?;
        let until = std::time::Instant::now() + Duration::from_secs_f64(secs);
        while std::time::Instant::now() < until {
            let _ = event.wait_for_event(200);
            render
                .write_to_device(&zeros, None)
                .map_err(|e| e.to_string())?;
        }
        let _ = client.stop_stream();
        Ok(format!(
            "held the device exclusively for {secs} s ({} Hz, {} ch, {} bit, digital zeros)",
            fmt.get_samplespersec(),
            fmt.get_nchannels(),
            fmt.get_validbitspersample()
        ))
    }

    pub struct Played {
        /// Render underruns seen (the buffer ran empty while signal remained).
        pub underruns: u64,
        /// Audio effects Windows reports on the render stream (Windows 11).
        pub effects: String,
    }

    /// Play `signal` (interleaved f32 in the device mix format) on the
    /// endpoint, then stop the stream completely. Blocks until played.
    pub fn play(id: Option<&str>, signal: &[f32]) -> Result<Played, String> {
        // The hard rule: nothing louder than -60 dBFS.
        let peak = signal.iter().fold(0.0f32, |m, &x| m.max(x.abs()));
        assert!(
            peak <= super::AMP && super::AMP < super::LOUDEST_ALLOWED,
            "test signal peak {peak} too loud"
        );
        let d = device(id)?;
        let mut client = d.get_iaudioclient().map_err(|e| e.to_string())?;
        let mix = client.get_mixformat().map_err(|e| e.to_string())?;
        if mix.get_subformat().ok() != Some(SampleType::Float) || mix.get_bitspersample() != 32 {
            return Err("the device mix format is not 32-bit float; the probe writes float".into());
        }
        let ch = mix.get_nchannels() as usize;
        client
            .initialize_client(
                &mix,
                Direction::Render,
                StreamMode::PollingShared {
                    autoconvert: false,
                    buffer_duration_hns: 2_000_000,
                },
                None,
            )
            .map_err(|e| format!("player: {e}"))?;
        let effects = match client
            .get_audio_effects_manager()
            .and_then(|m| m.get_audio_effects())
        {
            Ok(Some(list)) => {
                let on: Vec<String> = list
                    .iter()
                    .map(|e| {
                        format!(
                            "{} {}",
                            effect_name(e.id),
                            if e.state.0 == 1 { "ON" } else { "off" }
                        )
                    })
                    .collect();
                format!("{} reported: [{}]", list.len(), on.join(", "))
            }
            Ok(None) => "none reported".into(),
            Err(e) => format!("not available ({e})"),
        };
        let render = client.get_audiorenderclient().map_err(|e| e.to_string())?;
        let buffer = client.get_buffer_size().map_err(|e| e.to_string())? as usize;
        let total = signal.len() / ch;
        let mut pos = 0usize;
        let mut zeros_after = 0usize;
        let mut bytes: Vec<u8> = Vec::with_capacity(buffer * ch * 4);
        let mut underruns = 0u64;
        let mut started = false;
        loop {
            let avail = client
                .get_available_space_in_frames()
                .map_err(|e| e.to_string())? as usize;
            if started && avail == buffer && pos < total {
                underruns += 1;
            }
            if avail > 0 {
                bytes.clear();
                for i in 0..avail {
                    for c in 0..ch {
                        let v = if pos + i < total {
                            signal[(pos + i) * ch + c]
                        } else {
                            0.0
                        };
                        bytes.extend_from_slice(&v.to_le_bytes());
                    }
                }
                render
                    .write_to_device(&bytes, None)
                    .map_err(|e| e.to_string())?;
                let real = avail.min(total.saturating_sub(pos));
                zeros_after += avail - real;
                pos += real;
            }
            if !started {
                client.start_stream().map_err(|e| e.to_string())?;
                started = true;
            }
            if pos >= total {
                // Done once only trailing zeros remain queued.
                let padding = client.get_current_padding().map_err(|e| e.to_string())? as usize;
                if padding <= zeros_after {
                    break;
                }
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        client.stop_stream().map_err(|e| e.to_string())?;
        Ok(Played { underruns, effects })
    }
}

#[cfg(windows)]
fn run(args: Args) -> i32 {
    use jk_recorder::{Recorder, RecorderConfig, to_dbfs};
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
        let line = format!("LOOPBACK_PROBE result={result} {extra}");
        println!("{line}");
        if let Some(p) = &args.report
            && let Err(e) = std::fs::write(p, format!("{report}{line}\n"))
        {
            eprintln!("could not write report {}: {e}", p.display());
        }
        code
    };

    // COM for the probe's own player on this (main) thread.
    let _com = wasapi::initialize_mta();

    if args.list {
        match jk_recorder::capture_devices() {
            Ok(list) => {
                for d in list {
                    println!(
                        "{}{}  {}",
                        if d.is_default { "* " } else { "  " },
                        d.id,
                        d.name
                    );
                }
                return 0;
            }
            Err(e) => {
                eprintln!("{e}");
                return 1;
            }
        }
    }

    say!("jk-recorder loopback_probe {}", env!("CARGO_PKG_VERSION"));
    say!("os: {}", jk_recorder::os_version());
    let dev = args.device.as_deref();
    for f in win::facts(dev) {
        say!("{f}");
    }
    // Another app playing into the same device ends up in the capture (it is
    // the system mix), so bit-transparency can only be measured in quiet.
    // --wait-quiet waits for the other apps' audio sessions to go inactive.
    let mut others = win::active_sessions(dev);
    if args.wait_quiet > 0.0 && !others.is_empty() {
        let until = Instant::now() + Duration::from_secs_f64(args.wait_quiet);
        say!(
            "waiting up to {:.0} s for other apps to stop playing: {}",
            args.wait_quiet,
            others.join(", ")
        );
        while !others.is_empty() && Instant::now() < until {
            std::thread::sleep(Duration::from_secs(5));
            others = win::active_sessions(dev);
        }
        if !others.is_empty() {
            say!(
                "still playing after {:.0} s: {}",
                args.wait_quiet,
                others.join(", ")
            );
            return finish(
                &report,
                "busy",
                &format!("others=\"{}\"", others.join(", ")),
                7,
            );
        }
    }
    say!(
        "other apps with an active audio session on this device before the test: {}",
        if others.is_empty() {
            "none".to_string()
        } else {
            others.join(", ")
        }
    );

    let out = args.out.clone().unwrap_or_else(|| {
        std::env::temp_dir().join(format!("loopback_probe-{}.wav", std::process::id()))
    });
    let keep = args.out.is_some();
    let _ = std::fs::remove_file(&out);
    let mut config = RecorderConfig::new(&out);
    config.device_id = args.device.clone();
    config.silent_keepalive = args.keepalive;

    // --- record -----------------------------------------------------------
    let wall_start = Instant::now();
    let recorder = match Recorder::start(config) {
        Ok(r) => r,
        Err(e) => {
            say!("capture: could not start: {e}");
            return finish(&report, "error", &format!("reason=\"{e}\""), 1);
        }
    };
    let started_in = wall_start.elapsed();
    let st = recorder.status();
    let info = st
        .setup
        .device
        .clone()
        .unwrap_or(jk_recorder::OutputDeviceInfo {
            name: "unknown".into(),
            nominal_sample_rate: 0.0,
            physical_bits: None,
            physical_channels: None,
        });
    say!(
        "output device: {} | mix rate {} Hz | hardware {} bit, {} ch",
        info.name,
        info.nominal_sample_rate,
        info.physical_bits.map_or("?".into(), |b| b.to_string()),
        info.physical_channels.map_or("?".into(), |c| c.to_string())
    );
    for n in &st.setup.notes {
        say!("note: {n}");
    }
    let Some(format) = recorder.format() else {
        say!("capture: no format after start");
        return finish(&report, "error", "reason=\"no format\"", 1);
    };
    let rate = format.sample_rate;
    let ch = usize::from(format.channels);
    say!(
        "capture started in {:.0} ms: {format}",
        started_in.as_secs_f64() * 1000.0
    );

    let t0 = Instant::now();
    // Other apps that play into the device during the test (polled every
    // 250 ms while waiting): their sound is part of the system mix, so the
    // signal checks are not meaningful if any appear.
    let disturbed = std::cell::RefCell::new(Vec::<String>::new());
    let note_others = || {
        for o in win::active_sessions(dev) {
            let mut d = disturbed.borrow_mut();
            if !d.contains(&o) {
                d.push(o);
            }
        }
    };
    let sleep_until = |t: f64| {
        let target = t0 + Duration::from_secs_f64(t);
        loop {
            note_others();
            let now = Instant::now();
            if now >= target {
                break;
            }
            std::thread::sleep((target - now).min(Duration::from_millis(250)));
        }
    };
    let status_line = |label: &str, r: &Recorder| {
        let s = r.status();
        format!(
            "  [{label}] t={:.2}s state={:?} length={:.3}s blocks={} gaps={} peak dBFS={}",
            t0.elapsed().as_secs_f64(),
            s.state,
            s.stats.media_elapsed_secs,
            s.stats.blocks,
            s.stats.gap_events,
            s.levels
                .peak_dbfs()
                .iter()
                .map(|d| format!("{d:.1}"))
                .collect::<Vec<_>>()
                .join("/")
        )
    };

    if args.alloc_check {
        // Steady-state allocation check: nothing but the recorder runs in
        // this process for 5 s (this thread only sleeps); every heap
        // allocation in that window is counted (capture loop, pipeline,
        // writer and keep-alive threads together).
        use std::sync::atomic::Ordering;
        std::thread::sleep(Duration::from_secs(2));
        let a = alloc_count::ALLOCS.load(Ordering::Relaxed);
        let w = Instant::now();
        std::thread::sleep(Duration::from_secs(5));
        let b = alloc_count::ALLOCS.load(Ordering::Relaxed);
        let window = w.elapsed().as_secs_f64();
        let done = match recorder.stop() {
            Ok(d) => d,
            Err(e) => return finish(&report, "error", &format!("reason=\"{e}\""), 1),
        };
        if !keep {
            let _ = std::fs::remove_file(&out);
        }
        let blocks = done.stats.blocks;
        say!(
            "heap allocations in the whole process during {window:.2} s of steady recording: {} \
             ({blocks} packets in the whole recording)",
            b - a
        );
        let extra = format!("allocs={} window_s={window:.2} blocks={blocks}", b - a);
        return finish(
            &report,
            if b == a { "ok" } else { "allocations" },
            &extra,
            0,
        );
    }

    if args.grab_exclusive {
        // Device-lost test: 2 s of recording, then another stream takes the
        // device exclusively; the recording must stop visibly with a clear
        // message, and the file must be complete up to that moment.
        sleep_until(2.0);
        note_others();
        if !disturbed.borrow().is_empty() {
            say!(
                "not grabbing the device: other apps are using it ({})",
                disturbed.borrow().join(", ")
            );
            let _ = recorder.stop();
            return finish(&report, "busy", "", 7);
        }
        let grab_at = t0.elapsed().as_secs_f64();
        match win::grab_exclusive(dev, 1.0) {
            Ok(m) => say!("exclusive grab at t={grab_at:.2}s: {m}"),
            Err(e) => say!("exclusive grab at t={grab_at:.2}s failed: {e}"),
        }
        sleep_until(4.0);
        let st = recorder.status();
        say!("recorder state after the grab: {:?}", st.state);
        let file_now = jk_recorder::wav::read_header(&out);
        say!(
            "file on disk before stop(): {}",
            match &file_now {
                Ok(h) => format!(
                    "valid WAV, {} frames ({:.3} s)",
                    h.frames,
                    h.frames as f64 / f64::from(h.sample_rate)
                ),
                Err(e) => format!("not readable: {e}"),
            }
        );
        let done = match recorder.stop() {
            Ok(d) => d,
            Err(e) => {
                say!("stop() failed: {e}");
                return finish(&report, "error", &format!("reason=\"{e}\""), 1);
            }
        };
        for line in done.report().lines() {
            say!("{line}");
        }
        let failed = matches!(st.state, jk_recorder::RecorderState::Failed(_));
        let frames = done.file.map_or(0, |f| f.frames);
        let extra = format!(
            "state_failed={failed} file_frames={frames} file_s={:.3} grab_at_s={grab_at:.2} error=\"{}\"",
            frames as f64 / f64::from(rate),
            done.stats.error.clone().unwrap_or_default()
        );
        if !keep {
            let _ = std::fs::remove_file(&out);
        }
        if failed && file_now.is_ok() {
            say!("verdict: ok (the loss was reported and the file was closed)");
            return finish(&report, "ok", &extra, 0);
        }
        say!("verdict: the loss was not reported as expected");
        return finish(&report, "loss_not_reported", &extra, 8);
    }

    let ep_a = make_episode(EPISODE_A, rate, ch, 1);
    let ep_b = make_episode(EPISODE_B, rate, ch, 2);
    let mut underruns = 0u64;
    let mut effects = String::new();
    if !args.silence_only {
        sleep_until(LEAD_IN_S);
        say!("{}", status_line("lead-in silence done", &recorder));
        match win::play(dev, &ep_a.samples) {
            Ok(p) => {
                underruns += p.underruns;
                effects = p.effects;
            }
            Err(e) => {
                say!("player: {e}");
                let _ = recorder.stop();
                return finish(&report, "error", &format!("reason=\"player: {e}\""), 1);
            }
        }
        say!("{}", status_line("episode A played", &recorder));
        // What the device really played (loopback = after the endpoint
        // effects). If Windows raised it above -60 dBFS, play nothing more.
        let loudest = recorder
            .status()
            .levels
            .peak_total
            .iter()
            .fold(0.0f32, |m, &x| m.max(x));
        if loudest > LOUDEST_ALLOWED {
            say!(
                "the device output peaked at {:.2} dBFS (above -60 dBFS: Windows' endpoint effects \
                 raised the test signal); stopping without playing episode B",
                to_dbfs(loudest)
            );
            let _ = recorder.stop();
            return finish(
                &report,
                "too_loud",
                &format!("output_peak_dbfs={:.2}", to_dbfs(loudest)),
                1,
            );
        }
        let a_end = t0.elapsed().as_secs_f64();
        // Total silence: the player stream is stopped and released.
        sleep_until(a_end + args.silence);
        say!("{}", status_line("silence done", &recorder));
        match win::play(dev, &ep_b.samples) {
            Ok(p) => underruns += p.underruns,
            Err(e) => {
                say!("player: {e}");
                let _ = recorder.stop();
                return finish(&report, "error", &format!("reason=\"player: {e}\""), 1);
            }
        }
        say!("{}", status_line("episode B played", &recorder));
    }
    sleep_until(args.seconds);
    say!("{}", status_line("stopping", &recorder));
    let stop_call = Instant::now();
    let done = match recorder.stop() {
        Ok(d) => d,
        Err(e) => {
            say!("capture: could not stop cleanly: {e}");
            return finish(&report, "error", &format!("reason=\"{e}\""), 1);
        }
    };
    let wall_window = stop_call.duration_since(wall_start).as_secs_f64();
    say!("--- recorder report ---");
    for line in done.report().lines() {
        say!("{line}");
    }
    say!("--- probe checks ---");
    if !effects.is_empty() {
        say!("audio effects on the probe's render stream (IAudioEffectsManager): {effects}");
    }
    say!("player render underruns: {underruns}");

    let s = &done.stats;
    if let Some(e) = &s.error {
        say!("verdict: capture error: {e}");
        return finish(&report, "error", &format!("reason=\"{e}\""), 1);
    }
    let (wav, data) = match jk_recorder::wav::read_wav_f32(&out) {
        Ok(v) => v,
        Err(e) => {
            say!("could not read the capture: {e}");
            return finish(&report, "error", &format!("reason=\"read: {e}\""), 1);
        }
    };
    if !keep {
        let _ = std::fs::remove_file(&out);
    } else {
        say!("capture kept at {}", out.display());
    }
    let frames = wav.frames as usize;

    // 1. Sample rate.
    let rate_ok = wav.sample_rate == rate
        && (info.nominal_sample_rate == 0.0
            || f64::from(wav.sample_rate) == info.nominal_sample_rate);
    say!(
        "sample rate: file {} Hz, device mix {} Hz -> {}",
        wav.sample_rate,
        info.nominal_sample_rate,
        if rate_ok { "equal" } else { "DIFFERENT" }
    );

    // 2. Length against elapsed time.
    let span = s.start_to_stop_secs.unwrap_or(f64::NAN);
    let expected = span * f64::from(rate);
    let diff_frames = frames as f64 - expected;
    let packet = f64::from(rate) / 100.0; // one 10 ms engine period
    let ratio = frames as f64 / expected;
    let length_ok = diff_frames.abs() <= packet;
    say!(
        "length: {frames} frames = {:.6} s; Start..Stop on the OS clock {span:.6} s = {expected:.1} frames; \
         difference {diff_frames:+.1} frames ({:+.3} ms; one packet = {packet} frames); ratio {ratio:.6} -> {}",
        frames as f64 / f64::from(rate),
        diff_frames / f64::from(rate) * 1000.0,
        if length_ok {
            "within one packet"
        } else {
            "OFF BY MORE THAN ONE PACKET"
        }
    );
    say!(
        "wall clock: Recorder::start() call to stop() call {wall_window:.3} s (includes {:.0} ms of start-up \
         before the stream ran); recording {:.3} s",
        started_in.as_secs_f64() * 1000.0,
        frames as f64 / f64::from(rate)
    );
    say!(
        "timeline: {} frames from WASAPI packets, {} head fill, {} gap fill in {} gaps, {} tail fill, {} dropped",
        s.received_frames,
        s.head_fill_frames,
        s.gap_frames,
        s.gap_events,
        s.tail_fill_frames,
        s.dropped_frames
    );

    let mut extra = format!(
        "rate={} device_rate={} frames={frames} start_stop_s={span:.6} diff_frames={diff_frames:+.1} \
         ratio={ratio:.6} received={} head={} gaps={} gap_frames={} tail={} dropped={} keepalive={}",
        wav.sample_rate,
        info.nominal_sample_rate,
        s.received_frames,
        s.head_fill_frames,
        s.gap_events,
        s.gap_frames,
        s.tail_fill_frames,
        s.dropped_frames,
        args.keepalive
    );
    let disturbed = disturbed.into_inner();
    if !disturbed.is_empty() {
        extra.push_str(&format!(" disturbed_by=\"{}\"", disturbed.join(", ")));
    }
    if !rate_ok {
        say!("verdict: captured sample rate differs from the device");
        return finish(&report, "rate_mismatch", &extra, 3);
    }
    if !length_ok {
        say!("verdict: sample count does not match Start..Stop");
        return finish(&report, "count_mismatch", &extra, 4);
    }

    if args.silence_only {
        let peak = data.iter().fold(0.0f32, |m, &x| m.max(x.abs()));
        let nonzero = data.iter().filter(|&&x| x != 0.0).count();
        say!(
            "silence-only capture: peak {:.2} dBFS, {nonzero} of {} samples not exactly zero",
            to_dbfs(peak),
            data.len()
        );
        extra.push_str(&format!(
            " peak_dbfs={:.2} nonzero={nonzero}",
            to_dbfs(peak)
        ));
        if !disturbed.is_empty() {
            say!(
                "verdict: other apps played into the device during the test ({}); the length check \
                 above is valid, the silence check is not",
                disturbed.join(", ")
            );
            return finish(&report, "disturbed", &extra, 7);
        }
        say!("verdict: ok");
        return finish(&report, "ok", &extra, 0);
    }

    // 3. Find the episodes: the first sample at least half the signal level,
    // then refine with correlation on the first pseudo-random segment.
    let ch0: Vec<f32> = data.iter().step_by(ch).copied().collect();
    let thr = AMP * 0.5;
    let find_from = |from: usize| {
        ch0[from.min(ch0.len())..]
            .iter()
            .position(|x| x.abs() >= thr)
            .map(|p| p + from)
    };
    let mut worst_diff = 0.0f32;
    let mut nonidentical = 0usize;
    let mut compared = 0usize;
    let mut slips = 0usize;
    let mut gains = Vec::new();
    let mut tones = Vec::new();
    let mut ends = Vec::new();
    let mut starts = Vec::new();
    let mut search = 0usize;
    for (name, ep) in [("A", &ep_a), ("B", &ep_b)] {
        let Some(rough) = find_from(search) else {
            say!("episode {name}: not found in the capture");
            return finish(&report, "signal_missing", &extra, 5);
        };
        let tpl: Vec<f32> = ep.samples[..ep.seg[0].1.min(8192) * ch]
            .iter()
            .step_by(ch)
            .copied()
            .collect();
        let (pos, score) = align(&data, ch, rough, 64, &tpl);
        // Last pseudo-random segment must sit at the same offset (no slip).
        let tpl3: Vec<f32> = ep.samples
            [ep.seg[2].0 * ch..(ep.seg[2].0 + 8192.min(ep.seg[2].1 - ep.seg[2].0)) * ch]
            .iter()
            .step_by(ch)
            .copied()
            .collect();
        let (pos3, score3) = align(&data, ch, pos + ep.seg[2].0, 64, &tpl3);
        let slip = pos3 as i64 - (pos + ep.seg[2].0) as i64;
        if slip != 0 {
            slips += 1;
        }
        // Sample-by-sample comparison over the whole episode, all channels.
        let mut num = 0.0f64;
        let mut den = 0.0f64;
        let mut ep_worst = 0.0f32;
        let mut ep_diff = 0usize;
        for (i, &want) in ep.samples.iter().enumerate() {
            let got = data.get(pos * ch + i).copied().unwrap_or(f32::NAN);
            let d = (got - want).abs();
            if d.is_nan() || d > 0.0 {
                ep_diff += 1;
            }
            if d.is_nan() || d > ep_worst {
                ep_worst = if d.is_nan() { f32::INFINITY } else { d };
            }
            num += f64::from(got) * f64::from(want);
            den += f64::from(want) * f64::from(want);
        }
        let gain = num / den;
        worst_diff = worst_diff.max(ep_worst);
        nonidentical += ep_diff;
        compared += ep.frames * ch;
        gains.push(gain);
        // Sine: frequency and level from the middle of the sine segment.
        let (s0, s1) = ep.seg[1];
        let mid: Vec<f32> =
            ch0[pos + s0 + rate as usize / 10..pos + s1 - rate as usize / 10].to_vec();
        let hz = estimate_frequency(&mid, rate);
        let rms = (mid
            .iter()
            .map(|&x| f64::from(x) * f64::from(x))
            .sum::<f64>()
            / mid.len() as f64)
            .sqrt();
        tones.push((hz, 20.0 * rms.log10()));
        say!(
            "episode {name}: at frame {pos} ({:.3} s), correlation {score:.6}; last segment offset {slip:+} \
             frames (correlation {score3:.6}); {} of {} samples differ from what was played, max |diff| {:.3e} \
             ({}); least-squares gain {gain:.9}; sine {} Hz at {:.2} dBFS RMS (expected 997 Hz, {:.2} dBFS)",
            pos as f64 / f64::from(rate),
            ep_diff,
            ep.frames * ch,
            ep_worst,
            if ep_worst > 0.0 {
                format!(
                    "{:.1} dB below the signal peak",
                    20.0 * (f64::from(AMP) / f64::from(ep_worst)).log10()
                )
            } else {
                "bit-identical".into()
            },
            hz.map_or("n/a".into(), |f| format!("{f:.3}")),
            20.0 * rms.log10(),
            20.0 * (f64::from(AMP) / 2f64.sqrt()).log10()
        );
        starts.push(pos);
        ends.push(pos + ep.frames);
        search = pos + ep.frames + rate as usize / 2;
    }

    // 4. Everything outside the two episodes: silence, other apps, effects.
    let outside = |a: usize, b: usize| -> (f32, usize, usize) {
        let seg = &data[a.min(frames) * ch..b.min(frames) * ch];
        (
            seg.iter().fold(0.0f32, |m, &x| m.max(x.abs())),
            seg.iter().filter(|&&x| x != 0.0).count(),
            seg.len(),
        )
    };
    let (gap_peak, gap_nonzero, gap_len) = outside(ends[0], starts[1]);
    let (pre_peak, pre_nz, pre_len) = outside(0, starts[0]);
    let (post_peak, post_nz, post_len) = outside(ends[1], frames);
    say!(
        "silence between the episodes: {:.3} s, peak {} ({gap_nonzero} of {gap_len} samples not exactly 0)",
        (starts[1] - ends[0]) as f64 / f64::from(rate),
        if gap_peak > 0.0 {
            format!("{:.2} dBFS", to_dbfs(gap_peak))
        } else {
            "digital silence".into()
        }
    );
    say!(
        "before episode A: {:.3} s, peak {} ({pre_nz} of {pre_len} not 0); after episode B: {:.3} s, peak {} ({post_nz} of {post_len} not 0)",
        starts[0] as f64 / f64::from(rate),
        if pre_peak > 0.0 {
            format!("{:.2} dBFS", to_dbfs(pre_peak))
        } else {
            "digital silence".into()
        },
        (frames - ends[1]) as f64 / f64::from(rate),
        if post_peak > 0.0 {
            format!("{:.2} dBFS", to_dbfs(post_peak))
        } else {
            "digital silence".into()
        },
    );
    let bit_exact = nonidentical == 0;
    say!(
        "bit-transparency: {nonidentical} of {compared} samples differ; max |captured - played| = {worst_diff:.3e} -> {}",
        if bit_exact {
            "BIT-IDENTICAL"
        } else {
            "not bit-identical (Windows or another app changed the mix)"
        }
    );
    extra.push_str(&format!(
        " bit_exact={} max_abs_diff={worst_diff:.3e} differing={nonidentical}/{compared} gain_a={:.9} gain_b={:.9} \
         tone_a_hz={} tone_b_hz={} slips={slips} silence_s={:.3} silence_peak_dbfs={} underruns={underruns}",
        if bit_exact { "yes" } else { "no" },
        gains[0],
        gains[1],
        tones[0].0.map_or("n/a".into(), |f| format!("{f:.3}")),
        tones[1].0.map_or("n/a".into(), |f| format!("{f:.3}")),
        (starts[1] - ends[0]) as f64 / f64::from(rate),
        if gap_peak > 0.0 { format!("{:.2}", to_dbfs(gap_peak)) } else { "-inf".into() },
    ));
    if !disturbed.is_empty() {
        say!(
            "verdict: other apps played into the device during the test ({}); the length check above \
             is valid, the signal checks are not",
            disturbed.join(", ")
        );
        return finish(&report, "disturbed", &extra, 7);
    }
    if slips > 0 {
        say!("verdict: a dropout or sample slip inside an episode");
        return finish(&report, "dropout", &extra, 6);
    }
    say!("verdict: ok");
    finish(&report, "ok", &extra, 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn episode_layout_and_level() {
        let e = make_episode(EPISODE_A, 48_000, 2, 1);
        assert_eq!(e.frames, 4_5 * 48_000 / 10);
        assert_eq!(e.seg[1], (48_000, 168_000));
        let peak = e.samples.iter().fold(0.0f32, |m, &x| m.max(x.abs()));
        assert!(peak <= AMP);
        // PRBS segments are exactly +-AMP; channels differ.
        assert!(e.samples[..2 * 48_000].iter().all(|&x| x.abs() == AMP));
        assert_ne!(
            e.samples[0..64].iter().step_by(2).collect::<Vec<_>>(),
            e.samples[1..64].iter().step_by(2).collect::<Vec<_>>()
        );
    }

    #[test]
    fn alignment_finds_the_offset() {
        let e = make_episode(EPISODE_B, 48_000, 2, 2);
        let mut cap = vec![0.0f32; 1000 * 2];
        cap.extend_from_slice(&e.samples);
        let tpl: Vec<f32> = e.samples[..4096 * 2].iter().step_by(2).copied().collect();
        let (pos, score) = align(&cap, 2, 990, 64, &tpl);
        assert_eq!(pos, 1000);
        assert!((score - 1.0).abs() < 1e-9);
        let mid: Vec<f32> = e.samples[(48_000 + 4800) * 2..(144_000 - 4800) * 2]
            .iter()
            .step_by(2)
            .copied()
            .collect();
        let hz = estimate_frequency(&mid, 48_000).unwrap();
        assert!((hz - TONE_HZ).abs() < 0.01, "{hz}");
    }
}

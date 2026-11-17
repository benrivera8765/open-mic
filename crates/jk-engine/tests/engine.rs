// SPDX-License-Identifier: GPL-3.0-or-later
//! End-to-end tests of the engine API on the null device (no sound card needed):
//! real-time output equals the offline render, sample-accurate in-place seeks, pause
//! and resume, key changes without building Rubber Band on the render thread, device
//! loss, end of song, export formats, and the forward-compatibility rule for unknown
//! stem roles.

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};

use jk_dsp::{render, RenderOptions, StemsRef};
use jk_engine::{Engine, EngineEvent, ExportFormat, MixParams, PlaybackState};

fn tmp_dir() -> PathBuf {
    let d = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("jk-engine-tests");
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// The demo song, generated once per test binary.
fn demo() -> PathBuf {
    static ONCE: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
    ONCE.get_or_init(|| {
        let p = tmp_dir().join("demo.jksong");
        jk_engine::demo::make_demo_song(&p).unwrap();
        p
    })
    .clone()
}

/// Reference: decode every stem completely (jk-format's reference decoder, gains
/// applied) and run jk_dsp::render. Returns interleaved stereo.
fn reference_render(song: &Path, p: &MixParams) -> Vec<f32> {
    let f = jk_format::SongFile::open(song).unwrap();
    let m = f.manifest().clone();
    let frames = m.duration_frames as usize;
    let mut stems: Vec<[Vec<f32>; 2]> = Vec::new();
    for role in ["lead_vocal", "backing_vocals", "drums", "other"] {
        if m.stem(role).is_some() {
            let planes = jk_format::decode::decode_stem_f32(&f, role).unwrap();
            let r = planes.last().unwrap().clone();
            stems.push([planes[0].clone(), r]);
        } else {
            stems.push([vec![0.0; frames], vec![0.0; frames]]);
        }
    }
    let sref = StemsRef { sample_rate: m.sample_rate, frames, stems: std::array::from_fn(|s| [&stems[s][0][..], &stems[s][1][..]]) };
    let out = render(sref, p.to_dsp(), &[], &RenderOptions::native(m.sample_rate)).unwrap();
    let mut v = Vec::with_capacity(frames * 2);
    for i in 0..frames {
        v.push(out.channels[0][i]);
        v.push(out.channels[1][i]);
    }
    v
}

fn read_wav_f32(p: &Path) -> (u32, Vec<f32>) {
    let r = hound::WavReader::open(p).unwrap();
    let rate = r.spec().sample_rate;
    (rate, r.into_samples::<f32>().map(|x| x.unwrap()).collect())
}

fn export(song: &Path, p: &MixParams, fmt: ExportFormat, name: &str) -> (PathBuf, jk_engine::RenderReport) {
    let out = tmp_dir().join(name);
    let mut last = 0.0f32;
    let rep = jk_engine::render_offline(song, p, &out, fmt, &mut |x| {
        assert!(x >= last, "progress goes forward");
        last = x;
    }, &AtomicBool::new(false))
    .unwrap();
    assert_eq!(last, 1.0);
    (out, rep)
}

#[test]
fn export_wav_is_bit_identical_to_the_reference_render() {
    let song = demo();
    for semis in [0, 3, -5] {
        let p = MixParams { semitones: semis, guide_vocal: 0.3, ..MixParams::default() };
        let (out, rep) = export(&song, &p, ExportFormat::Wav32f, &format!("export_{semis}.wav"));
        let (rate, got) = read_wav_f32(&out);
        let want = reference_render(&song, &p);
        assert_eq!(rate, 44_100);
        assert_eq!(rep.frames as usize * 2, got.len());
        assert_eq!(got.len(), want.len());
        let diff = got.iter().zip(&want).filter(|(a, b)| a.to_bits() != b.to_bits()).count();
        assert_eq!(diff, 0, "key {semis}: {diff} samples differ from jk_dsp::render");
        assert_eq!(rep.applied_gain_db, 0.0);
        println!("export wav32f key {semis:+}: {} frames bit-identical to jk_dsp::render; peak {}", rep.frames, rep.peak);
    }
}

#[test]
fn export_flac24_is_the_render_within_24_bit_quantisation() {
    let song = demo();
    // A loud mix (levels up to 4x) so the render exceeds full scale and must be scaled.
    for (name, p) in [
        ("normal", MixParams { semitones: 2, ..MixParams::default() }),
        ("hot", MixParams { semitones: 2, drums: 4.0, other: 4.0, backing_level: 4.0, guide_vocal: 1.0, ..MixParams::default() }),
    ] {
        let (out, rep) = export(&song, &p, ExportFormat::Flac24, &format!("export_{name}.flac"));
        let want = reference_render(&song, &p);
        let (rate, planes) = jk_engine::FlacStem::decode_all(&out).unwrap();
        assert_eq!(rate, 44_100);
        assert_eq!(planes[0].len() * 2, want.len());
        let peak = want.iter().fold(0.0f32, |m, v| m.max(v.abs()));
        assert_eq!(rep.peak, peak);
        let gain = 10f64.powf(rep.applied_gain_db / 20.0);
        if name == "hot" {
            assert!(peak > 1.0 && rep.applied_gain_db < 0.0, "hot mix should be scaled, peak {peak}");
        } else {
            assert_eq!(rep.applied_gain_db, 0.0);
        }
        let mut max_err = 0.0f64;
        for i in 0..planes[0].len() {
            for c in 0..2 {
                let e = (planes[c][i] as f64 - want[i * 2 + c] as f64 * gain).abs();
                max_err = max_err.max(e);
            }
        }
        // Half an LSB of 24 bits (+ the 1e-12 slack of computing the gain from dB).
        assert!(max_err <= 0.5 / 8_388_608.0 + 1e-9, "{name}: max error {max_err}");
        let rep_v = jk_format::verify_file(&out, &Default::default());
        let _ = rep_v; // a bare FLAC is not a song file; MD5 is checked by render_offline
        println!(
            "export flac24 {name}: peak {peak:.4}, applied gain {:.3} dB, max |decoded - render x gain| = {max_err:.3e} (half LSB = {:.3e})",
            rep.applied_gain_db,
            0.5 / 8_388_608.0
        );
    }
}

#[test]
fn export_can_be_cancelled() {
    let song = demo();
    let out = tmp_dir().join("cancelled.flac");
    let cancel = AtomicBool::new(true);
    let e = jk_engine::render_offline(&song, &MixParams::default(), &out, ExportFormat::Flac24, &mut |_| {}, &cancel).unwrap_err();
    assert_eq!(e.code, "cancelled");
    assert!(!out.exists());
    let leftovers = std::fs::read_dir(tmp_dir()).unwrap().filter(|e| e.as_ref().unwrap().file_name().to_string_lossy().starts_with("cancelled.flac.tmp")).count();
    assert_eq!(leftovers, 0, "temporary file removed");
}

/// Wait until `cond` or panic after `secs`.
fn wait_for(secs: f64, what: &str, mut cond: impl FnMut() -> bool) {
    let t = Instant::now();
    while !cond() {
        assert!(t.elapsed().as_secs_f64() < secs, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(2));
    }
}

/// Fade-in length of the filler at `rate` (10 ms).
fn fade(rate: u32) -> usize {
    (rate / 100) as usize
}

#[test]
fn realtime_output_equals_offline_render() {
    let song = demo();
    for semis in [0, 4] {
        let p = MixParams { semitones: semis, ..MixParams::default() };
        let want = reference_render(&song, &p);
        let (engine, cap) = Engine::new_with_capture(44_100, true, 1.0).unwrap();
        let ev = engine.events();
        engine.load(&song, p).unwrap();
        engine.play();
        wait_for(60.0, "end of song", || engine.snapshot().state == PlaybackState::Ended);
        let got = cap.lock().unwrap().clone();
        // The capture starts with silence until the first segment is ready; the song then
        // plays from frame 0 with a 10 ms fade-in.
        let first = got.chunks(2).position(|f| f[0] != 0.0 || f[1] != 0.0).unwrap();
        let first_want = want.chunks(2).position(|f| f[0] != 0.0 || f[1] != 0.0).unwrap();
        let start = first - first_want; // capture frame of song frame 0
        let n = want.len() / 2;
        let g = &got[start * 2..(start + n) * 2];
        let f = fade(44_100);
        let mismatches = (f * 2..n * 2).filter(|&i| g[i].to_bits() != want[i].to_bits()).count();
        assert_eq!(mismatches, 0, "key {semis}: real-time output differs from the offline render");
        let d = engine.diagnostics();
        assert_eq!(d.underruns, 0);
        assert_eq!(d.inline_constructions, 0);
        let events: Vec<EngineEvent> = ev.try_iter().collect();
        assert!(events.contains(&EngineEvent::Ended), "Ended event");
        assert!((engine.snapshot().position_s - 40.0).abs() < 1e-9);
        println!("real-time (null device) key {semis:+}: {} frames after the fade-in bit-identical to jk_dsp::render", n - f);
    }
}

/// Find `want` in `got` (both interleaved stereo); returns the frame offset.
fn find(got: &[f32], want: &[f32]) -> Option<usize> {
    (0..(got.len() / 2).saturating_sub(want.len() / 2)).find(|&d| got[d * 2] == want[0] && got[d * 2 + 1] == want[1] && got[d * 2..d * 2 + want.len()] == *want)
}

#[test]
fn seek_is_in_place_and_sample_accurate() {
    let song = demo();
    let rate = 44_100u32;
    let f = fade(rate);
    for (name, p) in [
        ("key 0 full mix", MixParams::default()),
        ("key +3 drums only", MixParams { semitones: 3, backing_on: false, other: 0.0, ..MixParams::default() }),
    ] {
        let want = reference_render(&song, &p);
        let (engine, cap) = Engine::new_with_capture(rate, true, 1.0).unwrap();
        engine.load(&song, p).unwrap();
        engine.play();
        let mut lcg = 12345u64;
        for _ in 0..10 {
            lcg = lcg.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            // 1..29 s: every window there has sound in both cases (the drums stop at 36 s,
            // and an all-zero window would "match" any silence).
            let frame = ((lcg >> 33) % (28 * rate as u64)) as usize + rate as usize;
            // Seek while playing: the device keeps running (the capture keeps growing).
            let callbacks_before = engine.diagnostics().callbacks;
            cap.lock().unwrap().clear();
            engine.seek(frame as f64 / rate as f64);
            let compare = 16_384usize;
            wait_for(10.0, "audio after the seek", || cap.lock().unwrap().len() >= (frame_len_after(f, compare) + 20_000) * 2);
            let got = cap.lock().unwrap().clone();
            let w = &want[(frame + f) * 2..(frame + f + compare) * 2];
            assert!(w.iter().filter(|v| **v != 0.0).count() > 4000, "the comparison window has sound");
            let at = find(&got, w).unwrap_or_else(|| {
                let first = got.chunks(2).position(|f| f[0] != 0.0 || f[1] != 0.0).unwrap_or(0);
                let mut best = (usize::MAX, f32::MAX);
                for d in first.saturating_sub(2000)..(first + 2000).min(got.len() / 2 - w.len() / 2) {
                    let e = (0..w.len()).map(|i| (got[d * 2 + i] - w[i]).abs()).fold(0.0f32, f32::max);
                    if e < best.1 {
                        best = (d, e);
                    }
                }
                panic!("{name}: seek to frame {frame}: audio after the fade-in not found bit-exact; first sound at {first}, best offset {} max diff {}, capture {} frames", best.0, best.1, got.len() / 2)
            });
            // The fade-in covers exactly the f frames before: output = reference x ramp.
            for k in 0..f {
                for c in 0..2 {
                    let g = got[(at - f + k) * 2 + c] as f64;
                    let r = want[(frame + k) * 2 + c] as f64 * (k + 1) as f64 / f as f64;
                    assert!((g - r).abs() < 1e-5, "{name}: fade-in frame {k}");
                }
            }
            assert!(engine.diagnostics().callbacks > callbacks_before);
            assert_eq!(engine.snapshot().state, PlaybackState::Playing);
        }
        assert_eq!(engine.diagnostics().underruns, 0);
        println!("{name}: 10 seeks while playing, each bit-exact vs the offline render from the target frame (after a 10 ms fade-in)");
    }
}

fn frame_len_after(fade: usize, compare: usize) -> usize {
    fade + compare
}

#[test]
fn pause_resume_and_position() {
    let song = demo();
    // Real-time pacing so the clock is meaningful.
    let (engine, _cap) = Engine::new_with_capture(48_000, false, 1.0).unwrap();
    engine.load(&song, MixParams::default()).unwrap();
    engine.seek(10.0);
    assert!((engine.snapshot().position_s - 10.0).abs() < 1e-9, "position = seek target while paused");
    engine.play();
    std::thread::sleep(Duration::from_millis(600));
    let s1 = engine.snapshot();
    assert_eq!(s1.state, PlaybackState::Playing);
    assert!(s1.position_s > 10.2 && s1.position_s < 10.7, "position after 0.6 s: {}", s1.position_s);
    engine.pause();
    std::thread::sleep(Duration::from_millis(200));
    let p1 = engine.snapshot().position_s;
    std::thread::sleep(Duration::from_millis(300));
    let p2 = engine.snapshot().position_s;
    assert_eq!(p1, p2, "position holds while paused");
    engine.play();
    std::thread::sleep(Duration::from_millis(300));
    let p3 = engine.snapshot().position_s;
    assert!(p3 > p2 + 0.15 && p3 < p2 + 0.4, "resumes from the pause point: {p2} -> {p3}");
    assert_eq!(engine.diagnostics().underruns, 0);
}

#[test]
fn key_changes_use_prebuilt_rubber_band_instances() {
    let song = demo();
    let (engine, _cap) = Engine::new_with_capture(44_100, true, 1.0).unwrap();
    engine.load(&song, MixParams::default()).unwrap();
    engine.play();
    for k in [3, -2, 5, 0, -7, 12, -12, 2] {
        engine.set_params(MixParams { semitones: k, ..MixParams::default() });
        std::thread::sleep(Duration::from_millis(150));
    }
    wait_for(20.0, "key heard", || engine.diagnostics().audible_semitones == 2);
    let d = engine.diagnostics();
    assert_eq!(d.inline_constructions, 0, "no Rubber Band construction on the render thread");
    assert_eq!(d.shortfall_frames, 0);
    println!("8 key changes: 0 instances built on the render thread; waited for a pre-built instance in {} blocks", d.deferred_blocks);
}

#[test]
fn device_loss_pauses_and_play_reopens() {
    let song = demo();
    let (engine, _cap) = Engine::new_with_capture(48_000, false, 1.0).unwrap();
    let ev = engine.events();
    engine.load(&song, MixParams::default()).unwrap();
    engine.play();
    std::thread::sleep(Duration::from_millis(400));
    engine.debug_simulate_device_loss();
    wait_for(5.0, "pause after device loss", || engine.snapshot().state == PlaybackState::Paused);
    let pos = engine.snapshot().position_s;
    assert!(pos > 0.1, "position kept: {pos}");
    let events: Vec<EngineEvent> = ev.try_iter().collect();
    assert!(events.contains(&EngineEvent::DeviceLost));
    engine.play();
    std::thread::sleep(Duration::from_millis(400));
    let s = engine.snapshot();
    assert_eq!(s.state, PlaybackState::Playing);
    assert!(s.position_s > pos + 0.1, "plays on from {pos}: {}", s.position_s);
}

#[test]
fn set_device_switches_and_keeps_position() {
    let song = demo();
    let engine = Engine::new(jk_engine::EngineConfig { device_id: Some("null:44100".into()), exclusive: false, master: 0.0 }).unwrap();
    engine.load(&song, MixParams::default()).unwrap();
    engine.seek(20.0);
    engine.play();
    std::thread::sleep(Duration::from_millis(300));
    engine.set_device(Some("null:48000"), false).unwrap();
    let s = engine.snapshot();
    assert_eq!(s.state, PlaybackState::Playing);
    assert_eq!(s.device.as_ref().unwrap().sample_rate, 48_000);
    std::thread::sleep(Duration::from_millis(300));
    let p = engine.snapshot().position_s;
    assert!(p > 20.3 && p < 21.0, "position continues across the switch: {p}");
}

#[test]
fn unknown_roles_follow_the_forward_compatibility_rule() {
    // A song with only newer roles: "bass" (pitched, in the mix), "shaker" (unpitched,
    // in the mix) and "click" (not in the mix).
    let dir = tmp_dir();
    let rate = 44_100u32;
    let frames = 3 * rate as usize;
    let tone = |f: f64, a: f64| -> Vec<i32> {
        (0..frames).map(|i| ((a * (2.0 * std::f64::consts::PI * f * i as f64 / rate as f64).sin()) * 8_388_607.0).round() as i32).collect()
    };
    let make = |name: &str, roles: &[(&str, bool, bool, f64)]| -> PathBuf {
        let mut stems = Vec::new();
        let mut w_chunks = Vec::new();
        for (role, in_mix, pitched, f) in roles {
            let bytes = jk_engine::encode_flac(&tone(*f, 0.25), 1, 24, rate);
            w_chunks.push((format!("stem.{role}"), bytes));
            stems.push(serde_json::json!({"role": role, "chunk": format!("stem.{role}"), "codec": "flac", "bits": 24,
                "gain": 2.0, "in_karaoke_mix": in_mix, "pitched": pitched}));
        }
        let manifest: jk_format::Manifest = serde_json::from_value(serde_json::json!({
            "format": "jksong", "format_version": "1.7", "title": name, "artist": "test", "album": null,
            "sample_rate": rate, "channels": 1, "duration_frames": frames, "duration_seconds": 3.0,
            "cover": null, "lyrics": null, "analysis": null, "preset": "full", "stems_sum_to_source": false,
            "stems": stems, "provenance": {}
        }))
        .unwrap();
        let mut w = jk_format::SongWriter::new(manifest);
        for (id, b) in w_chunks {
            w.add_chunk(id, "audio/flac", jk_format::ChunkData::Bytes(b)).unwrap();
        }
        let p = dir.join(format!("{name}.jksong"));
        w.write_atomic(&p).unwrap();
        p
    };
    let expect_stem = |f: f64| -> Vec<f32> {
        tone(f, 0.25).iter().map(|&v| ((v as f32 / 8_388_608.0) as f64 * 2.0) as f32).collect()
    };
    // Pitched, in the mix: played at unity (with its stored gain) through the pitch bus.
    let p = make("bass", &[("bass", true, true, 110.0)]);
    let (out, _) = export(&p, &MixParams { drums: 0.3, other: 0.5, ..MixParams::default() }, ExportFormat::Wav32f, "bass.wav");
    let (_, got) = read_wav_f32(&out);
    let want = expect_stem(110.0);
    assert!(got.chunks(2).zip(&want).all(|(g, w)| g[0] == *w && g[1] == *w), "bass at unity, levels of other roles ignored");
    // Unpitched, in the mix, at +5: through the drum path, unshifted and exact.
    let p = make("shaker", &[("shaker", true, false, 3000.0)]);
    let (out, _) = export(&p, &MixParams { semitones: 5, ..MixParams::default() }, ExportFormat::Wav32f, "shaker.wav");
    let (_, got) = read_wav_f32(&out);
    let want = expect_stem(3000.0);
    let max = got.chunks(2).zip(&want).map(|(g, w)| (g[0] - w).abs().max((g[1] - w).abs())).fold(0.0f32, f32::max);
    assert!(max < 1e-6, "unpitched unknown role not shifted (max diff {max})");
    // Not in the mix: silent.
    let p = make("click", &[("click", false, true, 1000.0)]);
    let (out, _) = export(&p, &MixParams::default(), ExportFormat::Wav32f, "click.wav");
    let (_, got) = read_wav_f32(&out);
    assert!(got.iter().all(|&v| v == 0.0), "a role not in the karaoke mix is not played");
    // The engine reports what it does.
    let (engine, _cap) = Engine::new_with_capture(44_100, true, 1.0).unwrap();
    let loaded = engine.load(&p, MixParams::default()).unwrap();
    assert_eq!(loaded.stems, vec![jk_engine::StemInfo { role: "click".into(), in_karaoke_mix: false, pitched: true }]);
}

#[test]
fn errors_are_plain_and_never_panic() {
    let (engine, _cap) = Engine::new_with_capture(44_100, true, 1.0).unwrap();
    let e = engine.load(Path::new("does/not/exist.jksong"), MixParams::default()).unwrap_err();
    assert_eq!(e.code, "song_not_found");
    let junk = tmp_dir().join("junk.jksong");
    std::fs::write(&junk, b"not a song at all").unwrap();
    let e = engine.load(&junk, MixParams::default()).unwrap_err();
    assert!(e.code == "song_damaged" || e.code == "song_truncated", "{e}");
    // Calls without a song do nothing harmful.
    engine.play();
    engine.seek(3.0);
    engine.set_params(MixParams { semitones: 4, ..MixParams::default() });
    assert_eq!(engine.snapshot().state, PlaybackState::Empty);
}

#[test]
fn output_peak_and_clip_warning() {
    let song = demo();
    // Real-time pacing so the meter is read while playing.
    let (engine, _cap) = Engine::new_with_capture(48_000, false, 1.0).unwrap();
    engine.load(&song, MixParams::default()).unwrap();
    engine.seek(3.0);
    engine.play();
    std::thread::sleep(Duration::from_millis(700));
    let s = engine.snapshot();
    assert!(s.output_peak[0] > 0.05 && s.output_peak[0] < 1.0, "peak {:?}", s.output_peak);
    assert!(!s.clipped, "the default mix does not clip");
    // Levels at 4x drive the sum above full scale: flagged, never limited.
    engine.set_params(MixParams { drums: 4.0, other: 4.0, backing_level: 4.0, ..MixParams::default() });
    wait_for(5.0, "clip flag", || engine.snapshot().clipped);
    let s = engine.snapshot();
    assert!(s.output_peak[0] > 1.0, "peak above full scale is reported as is: {:?}", s.output_peak);
    // Turning the master down clears the warning; it stays clear at a safe level.
    engine.set_master(0.2);
    std::thread::sleep(Duration::from_millis(700));
    let s = engine.snapshot();
    assert!(!s.clipped, "warning cleared after the master change");
    assert!(s.output_peak[0] < 1.0, "peak includes the master volume: {:?}", s.output_peak);
    engine.pause();
    std::thread::sleep(Duration::from_millis(50));
    assert_eq!(engine.snapshot().output_peak, [0.0, 0.0], "no meter while paused");
}

#[test]
fn clip_warning_clears_when_the_mix_is_lowered() {
    // Verifier regression: the worker's 300 ms peak windows and the ~L frames of audio
    // still in the graph belong to the old mix; they must not re-latch the warning that
    // set_params just cleared.
    let song = demo();
    let (engine, _cap) = Engine::new_with_capture(48_000, false, 1.0).unwrap();
    engine.load(&song, MixParams::default()).unwrap();
    engine.seek(3.0);
    engine.play();
    std::thread::sleep(Duration::from_millis(300));
    engine.set_params(MixParams { drums: 4.0, other: 4.0, backing_level: 4.0, ..MixParams::default() });
    wait_for(5.0, "clip flag", || engine.snapshot().clipped);
    // The singer lowers the levels again: the default mix does not clip.
    engine.set_params(MixParams::default());
    std::thread::sleep(Duration::from_millis(1000));
    let s = engine.snapshot();
    assert!(s.output_peak[0] < 1.0, "the new mix is below full scale: {:?}", s.output_peak);
    assert!(!s.clipped, "warning stays cleared once the mix no longer clips");
}

#[test]
fn params_sent_without_a_song_do_not_leak_into_the_next_load() {
    // Verifier regression: a set_params with no song loaded (or no device) must not be
    // applied to the next song, which brings its own saved settings.
    let song = demo();
    let p = MixParams::default();
    let want = reference_render(&song, &p);
    let (engine, cap) = Engine::new_with_capture(44_100, true, 1.0).unwrap();
    engine.set_params(MixParams { guide_vocal: 1.0, other: 0.0, ..MixParams::default() });
    std::thread::sleep(Duration::from_millis(50));
    engine.load(&song, p).unwrap();
    engine.play();
    wait_for(60.0, "end of song", || engine.snapshot().state == PlaybackState::Ended);
    let got = cap.lock().unwrap().clone();
    let first = got.chunks(2).position(|f| f[0] != 0.0 || f[1] != 0.0).unwrap();
    let first_want = want.chunks(2).position(|f| f[0] != 0.0 || f[1] != 0.0).unwrap();
    assert!(first >= first_want, "capture starts later than the song");
    let start = first - first_want;
    let n = want.len() / 2;
    let g = &got[start * 2..(start + n) * 2];
    let f = fade(44_100);
    let mismatches = (f * 2..n * 2).filter(|&i| g[i].to_bits() != want[i].to_bits()).count();
    assert_eq!(mismatches, 0, "the song plays with the settings it was loaded with");
}

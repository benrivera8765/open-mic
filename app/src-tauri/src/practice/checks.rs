// SPDX-License-Identifier: GPL-3.0-or-later
//! The self-check's practice items (WO2 4.8; docs/APP_API.md 10.1), in this order after
//! "Key change": Microphone permission, Microphone capture, Pitch tracker, Practice guide,
//! Practice score, Practice lane. None makes a sound; the microphone capture is analysed in
//! memory and dropped (never written anywhere).

use std::fmt::Write as _;
use std::path::Path;
use std::time::{Duration, Instant};

use jk_pitch::score::{KeyTimeline, ScoreConfig};
use jk_recorder::input::{self as inp, MicPermission};
use serde::{Deserialize, Serialize};

use super::PracticeService;
use super::guide_view::SessionGuide;
use super::mic::{self, SourceSpec};
use super::synth;
use crate::guide::GuideStatus;
use crate::selfcheck::Item;

/// `LaneCost` (APP_API 10.1): the lane's drawing cost measured by the UI.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LaneCost {
    pub frames: u32,
    pub avg_ms: f64,
    pub p95_ms: f64,
    pub simple_avg_ms: f64,
    pub width: f64,
    pub height: f64,
    pub dpr: f64,
}

/// WO2 4.5's drawing budget (average ms per frame, the lyrics included).
pub const LANE_BUDGET_MS: f64 = 4.0;

fn item(name: &str, ok: bool, detail: impl Into<String>) -> Item {
    Item { name: name.to_string(), ok, detail: detail.into() }
}

/// 1. Microphone permission (never asks).
pub fn permission(svc: &PracticeService) -> Item {
    let p = svc.mic_permission();
    let mut d = format!("Microphone permission: {}.", p.state.replace('_', " "));
    if let Some(w) = &p.windows {
        let _ = write!(
            d,
            " Windows privacy switches (read only, never changed): microphone access {}, for this user {}, desktop apps {}.",
            w.microphone_access, w.user_access, w.desktop_apps
        );
    }
    let refused = matches!(p.state, "denied" | "restricted");
    if refused {
        let _ = write!(d, " {}", p.steps.unwrap_or_default());
    } else if p.state == "not_determined" {
        d.push_str(" macOS has not asked yet: practice mode explains and asks the first time it is opened.");
    }
    item("Microphone permission", !refused, d)
}

/// 2. A 3-second capture from the chosen (or automatic) input: format, levels, noise
/// floor; skipped with the reason when there is no permission or no input.
pub fn capture(svc: &PracticeService) -> Item {
    let name = "Microphone capture";
    let perm = svc.effective_permission();
    if perm != MicPermission::Granted {
        return item(name, true, format!("Skipped: the microphone permission is {}.", mic::permission_name(perm).replace('_', " ")));
    }
    let list = inp::list_input_devices().unwrap_or_default();
    let inactive = inp::list_inactive_inputs();
    let c = svc.chosen_input();
    let Some((dev, saved)) = inp::resolve_input(&list, c.device_id.as_deref(), c.device_name.as_deref()) else {
        let mut d = String::from("No input: this computer has no microphone or audio input connected (not a failure; practice opens as watch only).");
        if !list.is_empty() {
            let _ = write!(d, " Listed but never opened automatically (Bluetooth or software): {}.", list.iter().map(|d| d.name.as_str()).collect::<Vec<_>>().join(", "));
        }
        if !inactive.is_empty() {
            let _ = write!(d, " Inputs Windows knows but cannot use now: {}.", inactive.iter().map(|(n, s)| format!("{n} ({s})")).collect::<Vec<_>>().join(", "));
        }
        return item(name, true, d);
    };
    let spec = SourceSpec::Device { id: dev.id.clone() };
    let engine = std::sync::Arc::clone(svc.player().engine());
    let (mut stream, view) = match mic::open_source(&spec, &list, None, &engine) {
        Ok(x) => x,
        Err(e) => return item(name, false, format!("{} could not be opened: {}", dev.name, mic::plain_input_error(&e))),
    };
    let f = stream.format().clone();
    let t0 = Instant::now();
    let mut buf = Vec::new();
    while t0.elapsed() < Duration::from_secs(3) {
        while stream.read(&mut buf).is_some() {}
        std::thread::sleep(Duration::from_millis(10));
    }
    buf.iter_mut().for_each(|x| *x = 0.0);
    let lv = stream.levels();
    let health = stream.health();
    let state = stream.state();
    stream.stop();
    let mut d = format!(
        "{} ({}{}): {} Hz, {} channels, {}, declared latency {}; {:.1} s analysed in memory and dropped.",
        view.name,
        view.kind,
        if saved { ", chosen" } else { ", automatic choice" },
        f.sample_rate,
        f.channels,
        f.sample_format,
        f.declared_latency_ns.map_or("not declared".into(), |n| format!("{:.1} ms", n as f64 / 1e6)),
        lv.seconds
    );
    for (i, ch) in lv.channels.iter().enumerate() {
        let _ = write!(
            d,
            " Channel {}: peak {:.1} dBFS, RMS {:.1} dBFS, noise floor {}.",
            i + 1,
            ch.peak_dbfs.max(-120.0),
            ch.rms_dbfs.max(-120.0),
            ch.noise_floor_dbfs.map_or("not measured".into(), |x| format!("{x:.1} dBFS"))
        );
    }
    if let Some(s) = inp::suggest_voice_channel(&lv) {
        let _ = write!(d, " Suggested voice channel: {}.", s + 1);
    }
    let _ = write!(d, " Blocks {}, dropped {}.", health.blocks, health.dropped_blocks);
    let ok = health.blocks > 0 && !matches!(state, inp::InputState::Failed { .. });
    if !ok {
        let _ = write!(d, " The input delivered no sound: {state:?}. Check that no other app holds it and that it is switched on.");
    }
    item(name, ok, d)
}

/// 3. The tracker on generated tones C2..C6 and a generated voice through the real input
/// path, with its speed on this computer.
pub fn tracker() -> Item {
    let name = "Pitch tracker";
    let rate = 48_000u32;
    let notes = [36.0f64, 48.0, 60.0, 72.0, 84.0];
    let mut worst = 0.0f64;
    let mut octave_errors = 0usize;
    let mut delay_ms: Vec<f64> = Vec::new();
    let mut wall = 0.0f64;
    let mut audio = 0.0f64;
    let mut details = Vec::new();
    // A one-note "guide" per tone so the chain has something to map against.
    for &m in &notes {
        let hz = 440.0 * 2f64.powf((m - 69.0) / 12.0);
        let tone = inp::signal::VoiceTone { f0_hz: hz, peak_dbfs: -12.0, harmonics: 8, vibrato_hz: 0.0, vibrato_cents: 0.0 };
        let mut x = vec![0.0f32; (0.3 * f64::from(rate)) as usize];
        x.extend(tone.render(rate, 1.2));
        let guide = jk_pitch::score::ScoreGuide {
            notes: vec![jk_pitch::score::ScoreNote { start: 0.3, end: 1.5, midi: m, pitched: true, confidence: 1.0, octave_flagged: false, glide_in: 0.0, line: None }],
            low_confidence: 0.5,
            lines: Vec::new(),
            basis: jk_pitch::score::LineBasis::Phrases,
            outdated: false,
            duration_s: 1.5,
            label: "self-check".into(),
        };
        match synth::run_through_input(&guide, KeyTimeline::constant(0), x, rate, ScoreConfig::default()) {
            Ok(r) => {
                wall += r.wall_s;
                audio += r.audio_s;
                let steady: Vec<f64> = r.voiced.iter().filter(|(t, _)| *t > 0.6).map(|&(_, v)| v).collect();
                if steady.is_empty() {
                    details.push(format!("{}: no pitch", jk_pitch::score::note_name(m as i32, false)));
                    worst = f64::INFINITY;
                    continue;
                }
                let mut errs: Vec<f64> = steady.iter().map(|v| 100.0 * (v - m)).collect();
                octave_errors += errs.iter().filter(|e| e.abs() > 600.0).count();
                errs.sort_by(|a, b| a.total_cmp(b));
                let med = errs[errs.len() / 2];
                worst = worst.max(med.abs());
                if let Some(first) = r.voiced.iter().find(|(t, _)| *t >= 0.29).map(|(t, _)| *t) {
                    delay_ms.push((first - 0.3).max(0.0) * 1000.0 + r.first_value_at.unwrap_or(0.0) * 1000.0);
                }
                details.push(format!("{} {:+.1} c", jk_pitch::score::note_name(m as i32, false), med));
            }
            Err(e) => return item(name, false, format!("The generated input did not run: {e}")),
        }
    }
    let speed = if audio > 0.0 { 1000.0 * wall / audio } else { f64::NAN };
    delay_ms.sort_by(|a, b| a.total_cmp(b));
    let ok = worst <= 10.0 && octave_errors == 0;
    let d = format!(
        "Generated voice tones through the real input path (generated source, input rings, voice channel, tracker, song-time mapping): median error {} (worst {:.1} cents; target within 10), octave errors {octave_errors}; first value after the onset {}; speed {:.1} ms per second of audio on this computer (whole path, run as fast as possible).",
        details.join(", "),
        worst,
        delay_ms.first().map_or("not measured".into(), |d| format!("{d:.0} ms")),
        speed
    );
    item(name, ok, d)
}

/// The bundled demo song's guide: read from the file (or built when the file has none),
/// and built again from its lead stem to compare.
pub fn demo_guide(svc: &PracticeService, demo: Option<&Path>) -> (Item, Option<SessionGuide>) {
    let name = "Practice guide";
    let Some(path) = demo else { return (item(name, false, "The bundled demo song is missing from the app's resources."), None) };
    let song = match jk_format::SongFile::open(path) {
        Ok(s) => s,
        Err(e) => return (item(name, false, format!("The demo song could not be opened: {}", e.user_message())), None),
    };
    let stored = jk_format::guide::read_guide(&song).ok().flatten();
    let app = jk_guide::app_stamp("Open Mic self-check", env!("CARGO_PKG_VERSION"));
    let t0 = Instant::now();
    let built = jk_guide::build_for_song(path, &jk_pitch::guide::GuideConfig::default(), &app, &mut jk_guide::BuildControl::default());
    let took = t0.elapsed().as_secs_f64();
    let built = match built {
        Ok(b) => b,
        Err(e) => return (item(name, false, format!("A guide could not be built from the demo song's lead vocal: {}", e.message)), None),
    };
    let dur = song.manifest().duration();
    let speed = took / (dur / 60.0).max(1e-9);
    let counts = |d: &jk_format::guide::GuideDoc| {
        let c = d.compute_counts();
        format!("{} notes ({} pitched, {} unpitched, {} low confidence, {} octave flags, {} graded)", c.notes, c.pitched, c.unpitched, c.low_confidence, c.octave_flags, c.graded)
    };
    let (ok, d, doc, curve) = match stored {
        Some((doc, curve)) => {
            let same = doc.notes.len() == built.doc.notes.len()
                && doc.notes.iter().zip(&built.doc.notes).all(|(a, b)| (a.midi - b.midi).abs() < 0.05 && (a.start - b.start).abs() < 0.011 && a.kind == b.kind);
            let d = format!(
                "Read from the demo song: {}, made by {} {}; built again from its lead vocal in {:.2} s ({:.2} s per minute of audio on this computer): {}.",
                counts(&doc),
                doc.provenance.method,
                doc.provenance.version,
                took,
                speed,
                if same { "same notes".to_string() } else { format!("DIFFERENT notes ({})", counts(&built.doc)) }
            );
            (same, d, doc, curve)
        }
        None => {
            let d = format!(
                "The demo song carries no guide (a developer copy); one was built from its lead vocal in {:.2} s ({:.2} s per minute of audio): {}.",
                took,
                speed,
                counts(&built.doc)
            );
            (true, d, built.doc.clone(), Some(built.curve.clone()))
        }
    };
    let mut st = GuideStatus::from_file(&song);
    st.in_memory = st.state != "ready";
    let sg = SessionGuide::new("demo", &song, doc, curve, st);
    let _ = svc;
    (item(name, ok, d), Some(sg))
}

/// 5. Generated singers on the demo melody through the whole practice path: perfect in the
/// top band, a semitone flat in the lowest, silence nothing.
pub fn score(guide: Option<&SessionGuide>) -> Item {
    let name = "Practice score";
    let Some(g) = guide else { return item(name, false, "Skipped: no demo guide.") };
    let rate = 48_000u32;
    let keys = KeyTimeline::constant(0);
    let mut out = Vec::new();
    let mut ok = true;
    for (label, cents, want) in [("perfect", Some(0.0), "top"), ("a semitone flat", Some(-100.0), "lowest"), ("silent", None, "nothing")] {
        let x = synth::singer(&g.score, &keys, rate, cents, 0);
        match synth::run_through_input(&g.score, keys.clone(), x, rate, ScoreConfig::default()) {
            Ok(r) => {
                let o = r.result.overall;
                let pass = match want {
                    "top" => o.is_some_and(|v| v >= 90.0),
                    "lowest" => o.is_some_and(|v| v < 60.0),
                    _ => o.is_none_or(|v| v <= 1.0),
                };
                ok &= pass;
                out.push(format!(
                    "{label}: {} ({}, graded on {} of {} notes){}",
                    o.map_or("no grade".into(), |v| format!("{v:.1}")),
                    r.result.grade.clone().unwrap_or_else(|| "-".into()),
                    r.result.graded,
                    r.result.total,
                    if pass { "" } else { " - NOT in the expected band" }
                ));
            }
            Err(e) => return item(name, false, format!("The generated singer did not run: {e}")),
        }
    }
    item(name, ok, format!("Generated singers on the demo melody through the whole practice path: {}.", out.join("; ")))
}

/// 6. The lane's drawing cost in this web view (measured by the UI).
pub fn lane(cost: Option<&LaneCost>) -> Item {
    let name = "Practice lane";
    match cost {
        None => item(name, true, "Not measured (the page did not report the lane's drawing cost)."),
        Some(c) => {
            let ok = c.avg_ms <= LANE_BUDGET_MS;
            let mut d = format!(
                "Drawing the practice lane in this web view: {:.2} ms per frame on average, 95th percentile {:.2} ms, with the simple lane {:.2} ms ({} frames at {:.0}x{:.0}, pixel ratio {}); budget {LANE_BUDGET_MS} ms on average with the lyrics.",
                c.avg_ms, c.p95_ms, c.simple_avg_ms, c.frames, c.width, c.height, c.dpr
            );
            if !ok {
                d.push_str(" Over budget: switch on Settings > Practice > Simple lane on this computer.");
            }
            item(name, ok, d)
        }
    }
}

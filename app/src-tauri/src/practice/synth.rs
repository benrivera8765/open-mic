// SPDX-License-Identifier: GPL-3.0-or-later
//! Generated singers and an offline run of the whole input path, for the self-check's
//! practice items (WO2 4.8, APP_API 10.1) and the app's tests. Generated sound only:
//! nothing here opens a microphone, records anything or plays a sound.
//!
//! The singer is the harness's "natural" scripted singer (`jukeoke-cli practice run
//! --singer`): the guide's pitched notes with a short scoop into each note and a gentle
//! vibrato on held notes, made by the tracker's synthetic voice (`jk_pitch::tracker::
//! testvoice`), optionally off by some cents or octaves.
//!
//! [`run_through_input`] feeds such a signal through the very path a microphone takes in
//! the app: a `jk_recorder::input::SignalSource` (delivering blocks with host-clock stamps,
//! as fast as they are read) -> the input rings and meters (`InputStream`) -> the voice
//! channel -> `jk_pitch::practice::Chain` (tracker, song-time mapping by playback anchors,
//! scorer).

use std::time::{Duration, Instant};

use jk_pitch::practice::{Chain, ChainConfig, ClockAnchor, PracticeLive};
use jk_pitch::score::{KeyTimeline, ScoreConfig, ScoreGuide, ScoreNote, ScoreResult};
use jk_pitch::tracker::testvoice::{self as tv, Voice, Vowel};
use jk_recorder::input::{InputState, InputStream, Pacing, SignalSource};

/// A scripted singer for the whole guide at `rate`: the guide's notes in the key of
/// `keys`, off by `cents` and `octave` octaves (`None`: silence).
pub fn singer(guide: &ScoreGuide, keys: &KeyTimeline, rate: u32, cents: Option<f64>, octave: i32) -> Vec<f32> {
    let dur = guide.duration_s;
    let Some(cents) = cents else { return tv::silence(rate, dur) };
    let voice = Voice { vowel: Vowel::Ah, level_dbfs: -20.0, breath_db: None, seed: 17 };
    let notes: Vec<ScoreNote> = guide.notes.iter().filter(|n| n.pitched).cloned().collect();
    let keys = keys.clone();
    tv::sing(rate, dur, voice, move |t| {
        let i = notes.partition_point(|n| n.end <= t);
        let n = notes.get(i).filter(|n| t >= n.start)?;
        let into = t - n.start;
        let len = n.end - n.start;
        let mut m = n.midi + f64::from(keys.key_at(t, None)) + cents / 100.0 + 12.0 * f64::from(octave);
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

/// What an offline run through the input path gave.
pub struct OfflineRun {
    pub result: ScoreResult,
    /// Tracker values with a pitch: (input time s, MIDI).
    pub voiced: Vec<(f64, f64)>,
    /// Values without a pitch.
    pub unvoiced: usize,
    /// Wall time of the whole run (s) and the audio it covered (s).
    pub wall_s: f64,
    pub audio_s: f64,
    /// Input time (s) processed when the first pitch value came out, per onset asked for
    /// (the tracker's delay from sound to value).
    pub first_value_at: Option<f64>,
}

/// Plays `mono` (at `rate`) through a generated input source, the rings and the chain as
/// if a microphone delivered it while the song played from 0 (song time = input time).
pub fn run_through_input(guide: &ScoreGuide, keys: KeyTimeline, mono: Vec<f32>, rate: u32, score: ScoreConfig) -> Result<OfflineRun, String> {
    let audio_s = mono.len() as f64 / f64::from(rate);
    let src = SignalSource::new("generated singer", rate, 1, mono).pacing(Pacing::AsFastAsPossible);
    let mut stream = InputStream::open(Box::new(src)).map_err(|e| e.message)?;
    let mut cfg = ChainConfig::new(rate, 1, 0);
    cfg.reference_channels = 0;
    cfg.score = score;
    let mut chain = Chain::new(cfg, guide.clone(), keys, Vec::new());
    let mut live = PracticeLive::default();
    let mut buf = Vec::new();
    let mut voiced = Vec::new();
    let mut unvoiced = 0usize;
    let mut t0: Option<i64> = None;
    let mut consumed = 0usize;
    let mut first_value_at = None;
    let started = Instant::now();
    loop {
        match stream.read(&mut buf) {
            Some(st) => {
                let base = *t0.get_or_insert(st.host_ns);
                // The playback clock: song time = input time from the first frame on.
                chain.playback_anchor(ClockAnchor { host_ns: st.host_ns, song_s: (st.host_ns - base) as f64 / 1e9, segment: 1, playing: true });
                chain.process(st.host_ns, &buf, &[]);
                consumed += buf.len();
                chain.drain_live(&mut live);
                for &(t, m, _) in &live.pitch {
                    match m {
                        Some(m) => {
                            if first_value_at.is_none() {
                                first_value_at = Some(consumed as f64 / f64::from(rate) - t.max(0.0));
                            }
                            voiced.push((t, m));
                        }
                        None => unvoiced += 1,
                    }
                }
            }
            None => match stream.state() {
                InputState::Ended | InputState::Failed { .. } | InputState::Stopped => break,
                _ => {
                    if started.elapsed() > Duration::from_secs(120) {
                        return Err("the generated input did not finish within 2 minutes".into());
                    }
                    std::thread::sleep(Duration::from_millis(1));
                }
            },
        }
    }
    let result = chain.finish(true, &[]);
    Ok(OfflineRun { result, voiced, unvoiced, wall_s: started.elapsed().as_secs_f64(), audio_s, first_value_at })
}

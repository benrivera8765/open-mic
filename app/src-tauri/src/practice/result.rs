// SPDX-License-Identifier: GPL-3.0-or-later
//! A graded take as it is stored in the song's slot (SONG_FORMAT section 11, through
//! `jk_format::practice`, which applies the number-precision rules) and as the result
//! screen gets it (docs/APP_API.md 12.6 `PracticeResult`: the stored fields plus
//! `trust_messages`, `best`, `saved`, `key_label`).

use jk_format::practice::{LineBreakdown, LineRow, PracticeResult};
use jk_pitch::score::{LineBasis, ScoreResult};
use serde_json::{Map, Value, json};

use crate::keys::Key;

/// A take that covered less of the song's graded note time than this is not kept in the
/// history (APP_API 12.2).
pub const MIN_SAVED_SHARE: f64 = 0.10;

/// The stored result of a scorer's result, dated now.
pub fn stored(r: &ScoreResult, app_version: &str) -> PracticeResult {
    PracticeResult {
        date: jk_format::time::utc_now_iso8601(),
        key: r.key.clamp(-12, 12),
        key_changes: r.key_changes,
        seeks: r.seeks,
        octave: r.octave.clamp(-2, 2),
        octave_mode: r.octave_mode.name().to_string(),
        strictness: r.strictness.name().to_string(),
        overall: r.overall,
        grade: r.grade.clone(),
        graded: r.graded,
        total: r.total,
        tendency_cents: r.tendency_cents,
        sung_share: r.sung_share,
        voiced_share: r.voiced_share,
        completed: r.completed,
        trust: r.trust.clone(),
        guide: r.guide.clone(),
        scorer: r.scorer.clone(),
        app: app_version.to_string(),
        lines: Some(LineBreakdown {
            basis: match r.lines.basis {
                LineBasis::Lyrics => "lyrics".into(),
                LineBasis::Phrases => "phrases".into(),
            },
            rows: r.lines.rows.iter().map(|&(l, s, sc, g, t)| LineRow(l, s, sc, g, t)).collect(),
            extra: Map::new(),
        }),
        extra: Map::new(),
    }
    .rounded()
}

/// One plain sentence per trust flag (SONG_FORMAT 11 `trust`); unknown flags are shown as
/// they are.
pub fn trust_message(flag: &str) -> String {
    match flag {
        "music_in_input" => "The mixer sent the app's own music back into the microphone input during this take.".into(),
        "removal_active" => "The app removed its own music from the input well, so the grade counts normally.".into(),
        "removal_marginal" => "The app could only partly remove its own music from the input, so this grade is less certain.".into(),
        "untrusted" => "The music coming back in the input could not be removed (its delay kept changing, as it does over \
                        Bluetooth), so this take could not be graded. Practise with the music on a cable or over USB."
            .into(),
        "low_level" => "Your voice was too quiet for a reliable pitch for part of the take. Turn up the microphone's gain on \
                        the mixer."
            .into(),
        "clipping" => "The input was too loud and clipped. Turn down the microphone's gain on the mixer.".into(),
        "uncertain_guide" => "The practice data for this song is unsure in places, so the grade may be less reliable.".into(),
        "latency_unmeasured" => "The delay of your equipment could not be measured, so small timing differences were forgiven \
                                 instead."
            .into(),
        "input_lost" => "The input stopped during the take (unplugged or switched off); the time without it counts as not \
                         sung."
            .into(),
        "dropouts" => "Some sound from the input was lost because the computer was busy.".into(),
        "bluetooth_input" => "A Bluetooth microphone was used: its delay wanders, so timing and pitch may be less reliable.".into(),
        "stopped_early" => "The take ended before the song did: it is graded on what was sung.".into(),
        other => other.to_string(),
    }
}

/// The result screen's view: the stored fields plus `trust_messages`, `best`, `saved` and
/// `key_label` (APP_API 12.6).
pub fn screen_view(r: &PracticeResult, best: bool, saved: bool, song_key: Option<Key>) -> Value {
    let mut v = serde_json::to_value(r).unwrap_or_else(|_| json!({}));
    if let Some(o) = v.as_object_mut() {
        // A stored result always has `lines` (an older one may not): the screen expects it.
        if o.get("lines").is_none_or(Value::is_null) {
            o.insert("lines".into(), json!({"basis": "phrases", "rows": []}));
        }
        o.insert("trust_messages".into(), json!(r.trust.iter().map(|f| trust_message(f)).collect::<Vec<_>>()));
        o.insert("best".into(), json!(best));
        o.insert("saved".into(), json!(saved));
        o.insert("key_label".into(), json!(crate::keys::key_label(song_key, r.key)));
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_known_flag_has_a_sentence() {
        for f in [
            "music_in_input",
            "removal_active",
            "removal_marginal",
            "untrusted",
            "low_level",
            "clipping",
            "uncertain_guide",
            "latency_unmeasured",
            "input_lost",
            "dropouts",
            "bluetooth_input",
            "stopped_early",
        ] {
            let m = trust_message(f);
            assert!(m.len() > 30 && m.ends_with('.'), "{f}: {m}");
        }
        assert_eq!(trust_message("from_the_future"), "from_the_future");
    }
}

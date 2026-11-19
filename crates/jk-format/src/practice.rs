// SPDX-License-Identifier: GPL-3.0-or-later
//! The singer's practice data in the settings slot (SONG_FORMAT.md section 11, 1.1): the
//! user-state key `practice` with the octave choice, the excluded lines and the results.
//!
//! The value lives in [`UserState::extra`] under `practice` and is parsed on demand
//! ([`UserState::practice`]), so a reader that does not use it (or a newer shape it does
//! not understand) never changes it. Writers go through [`Practice::rounded`] (the
//! number-precision rules are part of the format: unrounded doubles would make the
//! worst case overflow a slot) and [`save_practice`], which never rewrites the whole
//! file for practice data: when the state would not fit a slot it drops the rows of the
//! oldest `recent` results, then the oldest `recent` results, never `best`.

use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::error::{ErrorKind, FormatError, Result};
use crate::guide::round_to;
use crate::serde_util::*;
use crate::song::{SlotWrite, update_user_state};
use crate::user_state::UserState;

/// The user-state key.
pub const PRACTICE_KEY: &str = "practice";
/// `practice_version` written by 1.1 writers.
pub const PRACTICE_VERSION: u32 = 1;
/// At most this many `recent` results are kept.
pub const MAX_RECENT: usize = 20;

fn one_u32() -> u32 {
    1
}
fn auto() -> String {
    "auto".into()
}

/// The lane's octave choice (section 11).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct OctaveChoice {
    /// `auto`, `fixed` or `strict`.
    #[serde(default = "auto", deserialize_with = "lenient_mode")]
    pub mode: String,
    /// -2..2: the shift in use (in `auto` the one found last time).
    #[serde(default, deserialize_with = "lenient_i32")]
    pub shift: i32,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

fn lenient_mode<'de, D: serde::Deserializer<'de>>(d: D) -> std::result::Result<String, D::Error> {
    Ok(Value::deserialize(d)?.as_str().map(str::to_string).unwrap_or_else(auto))
}

impl Default for OctaveChoice {
    fn default() -> Self {
        OctaveChoice { mode: auto(), shift: 0, extra: Map::new() }
    }
}

/// A line the singer marked "the guide is wrong here": a time span (what counts) and the
/// lyrics line it was made from (display only).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Excluded {
    #[serde(default, deserialize_with = "lenient")]
    pub line: Option<i64>,
    #[serde(deserialize_with = "lenient_f64")]
    pub start: f64,
    #[serde(deserialize_with = "lenient_f64")]
    pub end: f64,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// The line-by-line breakdown of a result.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct LineBreakdown {
    /// `lyrics` or `phrases`.
    #[serde(default)]
    pub basis: String,
    /// `[line, start, score, graded, tendency]` per row.
    #[serde(default)]
    pub rows: Vec<LineRow>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// One row `[line, start, score, graded, tendency]` (a JSON array). Score and tendency
/// are whole numbers in the file (written as JSON integers; a float from another writer
/// reads fine).
#[derive(Clone, Copy, Debug, PartialEq, Deserialize)]
pub struct LineRow(pub i64, pub f64, pub Option<f64>, pub u32, pub Option<f64>);

impl Serialize for LineRow {
    fn serialize<S: serde::Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        use serde::ser::SerializeTuple;
        let mut t = s.serialize_tuple(5)?;
        t.serialize_element(&self.0)?;
        t.serialize_element(&self.1)?;
        t.serialize_element(&Whole(self.2))?;
        t.serialize_element(&self.3)?;
        t.serialize_element(&Whole(self.4))?;
        t.end()
    }
}

/// Writes a whole number as a JSON integer (`-12`, not `-12.0`): the format's whole-number
/// fields stay as short as the size bound of section 11 assumes.
struct Whole(Option<f64>);

impl Serialize for Whole {
    fn serialize<S: serde::Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        match self.0 {
            Some(x) if x.is_finite() && x.fract() == 0.0 && x.abs() < 9.0e15 => s.serialize_i64(x as i64),
            Some(x) => s.serialize_f64(x),
            None => s.serialize_none(),
        }
    }
}

fn ser_whole<S: serde::Serializer>(v: &Option<f64>, s: S) -> std::result::Result<S::Ok, S::Error> {
    Whole(*v).serialize(s)
}

/// One graded take (section 11 "A result").
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PracticeResult {
    pub date: String,
    pub key: i32,
    #[serde(default)]
    pub key_changes: u32,
    #[serde(default)]
    pub seeks: u32,
    #[serde(default)]
    pub octave: i32,
    #[serde(default = "auto")]
    pub octave_mode: String,
    #[serde(default)]
    pub strictness: String,
    /// 0..100, one decimal; `null` when the take could not be graded honestly.
    pub overall: Option<f64>,
    pub grade: Option<String>,
    pub graded: u32,
    pub total: u32,
    /// Whole cents, positive = sharp; `null` if too little was sung.
    #[serde(serialize_with = "ser_whole")]
    pub tendency_cents: Option<f64>,
    /// Two decimals.
    pub sung_share: f64,
    /// Two decimals.
    pub voiced_share: f64,
    pub completed: bool,
    #[serde(default)]
    pub trust: Vec<String>,
    #[serde(default)]
    pub guide: String,
    #[serde(default)]
    pub scorer: String,
    #[serde(default)]
    pub app: String,
    #[serde(default)]
    pub lines: Option<LineBreakdown>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl PracticeResult {
    /// The number-precision rules of section 11: `overall` one decimal; `sung_share` and
    /// `voiced_share` two decimals; `tendency_cents` and every row's score and tendency
    /// whole numbers; row `start` one decimal.
    pub fn rounded(&self) -> PracticeResult {
        let mut r = self.clone();
        r.overall = r.overall.map(|x| round_to(x, 1));
        r.sung_share = round_to(r.sung_share, 2);
        r.voiced_share = round_to(r.voiced_share, 2);
        r.tendency_cents = r.tendency_cents.map(|x| round_to(x, 0));
        if let Some(l) = r.lines.as_mut() {
            for row in &mut l.rows {
                row.1 = round_to(row.1, 1);
                row.2 = row.2.map(|x| round_to(x, 0));
                row.4 = row.4.map(|x| round_to(x, 0));
            }
        }
        r
    }

    /// Counts toward `best`: `sung_share >= 0.9` and not `untrusted`.
    pub fn eligible_for_best(&self) -> bool {
        self.sung_share >= 0.9 && self.overall.is_some() && !self.trust.iter().any(|t| t == "untrusted")
    }
}

/// The `practice` value (section 11).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Practice {
    #[serde(default = "one_u32")]
    pub practice_version: u32,
    #[serde(default, deserialize_with = "lenient")]
    pub octave: OctaveChoice,
    /// `relaxed`, `standard`, `strict`, or `null` = the app's setting.
    #[serde(default, deserialize_with = "lenient")]
    pub strictness: Option<String>,
    #[serde(default, deserialize_with = "lenient")]
    pub excluded: Vec<Excluded>,
    /// Reserved; version 1 defines no keys.
    #[serde(default, deserialize_with = "lenient")]
    pub lane: Map<String, Value>,
    #[serde(default, deserialize_with = "lenient_result")]
    pub best: Option<PracticeResult>,
    /// Newest first, at most 20.
    #[serde(default, deserialize_with = "lenient_results")]
    pub recent: Vec<PracticeResult>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

fn lenient_result<'de, D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Option<PracticeResult>, D::Error> {
    Ok(serde_json::from_value(Value::deserialize(d)?).ok())
}

/// Results that do not parse are dropped (one bad entry must not lose the others).
fn lenient_results<'de, D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Vec<PracticeResult>, D::Error> {
    Ok(match Value::deserialize(d)? {
        Value::Array(a) => a.into_iter().filter_map(|v| serde_json::from_value(v).ok()).collect(),
        _ => Vec::new(),
    })
}

impl Default for Practice {
    fn default() -> Self {
        Practice {
            practice_version: PRACTICE_VERSION,
            octave: OctaveChoice::default(),
            strictness: None,
            excluded: Vec::new(),
            lane: Map::new(),
            best: None,
            recent: Vec::new(),
            extra: Map::new(),
        }
    }
}

impl Practice {
    /// Every result rounded by the precision rules, at most [`MAX_RECENT`] recent ones.
    pub fn rounded(&self) -> Practice {
        let mut p = self.clone();
        p.best = p.best.as_ref().map(PracticeResult::rounded);
        p.recent.truncate(MAX_RECENT);
        p.recent = p.recent.iter().map(PracticeResult::rounded).collect();
        p
    }

    /// Adds a finished take: newest first in `recent` (at most 20) and `best` updated by
    /// the rule of section 11 (highest `overall` among eligible results).
    pub fn add_result(&mut self, r: PracticeResult) {
        let r = r.rounded();
        if r.eligible_for_best()
            && self.best.as_ref().is_none_or(|b| r.overall.unwrap_or(f64::MIN) > b.overall.unwrap_or(f64::MIN))
        {
            self.best = Some(r.clone());
        }
        self.recent.insert(0, r);
        self.recent.truncate(MAX_RECENT);
    }

    /// One step of the size rule: drops the `rows` of the oldest `recent` result that
    /// still has rows, else the oldest `recent` result. Returns false when nothing is left
    /// to drop (`best` is never dropped).
    pub fn shrink_once(&mut self) -> bool {
        if let Some(r) = self.recent.iter_mut().rev().find(|r| r.lines.as_ref().is_some_and(|l| !l.rows.is_empty())) {
            if let Some(l) = r.lines.as_mut() {
                l.rows.clear();
            }
            return true;
        }
        self.recent.pop().is_some()
    }
}

impl UserState {
    /// The `practice` value, parsed. `None` when absent; a value that is not an object
    /// also reads as `None` (and is left untouched in the file).
    pub fn practice(&self) -> Option<Practice> {
        let v = self.extra.get(PRACTICE_KEY)?;
        if !v.is_object() {
            return None;
        }
        serde_json::from_value(v.clone()).ok()
    }

    /// Stores `p` (rounded by the precision rules) under `practice`.
    pub fn set_practice(&mut self, p: &Practice) {
        let v = serde_json::to_value(p.rounded()).expect("practice data always serialises");
        self.extra.insert(PRACTICE_KEY.into(), v);
    }
}

/// Checks a `practice` value against the types of section 11 (`jukeoke-cli verify`).
/// Unknown keys are allowed.
pub fn validate_practice(v: &Value) -> Vec<String> {
    let mut e = Vec::new();
    let Some(o) = v.as_object() else {
        return vec!["practice: not a JSON object".into()];
    };
    if let Some(oc) = o.get("octave") {
        match oc.as_object() {
            Some(oc) => {
                if !matches!(oc.get("mode").and_then(Value::as_str), Some("auto" | "fixed" | "strict")) {
                    e.push("practice: octave.mode must be auto, fixed or strict".into());
                }
                if !oc.get("shift").and_then(Value::as_i64).is_some_and(|s| (-2..=2).contains(&s)) {
                    e.push("practice: octave.shift must be an integer -2..2".into());
                }
            }
            None => e.push("practice: octave must be an object".into()),
        }
    }
    match o.get("strictness") {
        None | Some(Value::Null) => {}
        Some(Value::String(s)) if matches!(s.as_str(), "relaxed" | "standard" | "strict") => {}
        Some(_) => e.push("practice: strictness must be relaxed, standard, strict or null".into()),
    }
    if let Some(x) = o.get("excluded") {
        match x.as_array() {
            Some(a) => {
                for (i, s) in a.iter().enumerate() {
                    let ok = s.get("start").and_then(Value::as_f64).zip(s.get("end").and_then(Value::as_f64)).is_some_and(|(a, b)| a <= b);
                    if !ok {
                        e.push(format!("practice: excluded[{i}] needs numbers start <= end"));
                    }
                }
            }
            None => e.push("practice: excluded must be a list".into()),
        }
    }
    let check_result = |r: &Value, who: &str, e: &mut Vec<String>| {
        if serde_json::from_value::<PracticeResult>(r.clone()).is_err() {
            e.push(format!("practice: {who} is not a valid result"));
            return;
        }
        if let Some(x) = r.get("overall").and_then(Value::as_f64) {
            if !(0.0..=100.0).contains(&x) {
                e.push(format!("practice: {who}.overall {x} not within 0..100"));
            }
        }
        if let Some(k) = r.get("key").and_then(Value::as_i64) {
            if !(-12..=12).contains(&k) {
                e.push(format!("practice: {who}.key {k} not within -12..12"));
            }
        }
    };
    match o.get("best") {
        None | Some(Value::Null) => {}
        Some(b) => check_result(b, "best", &mut e),
    }
    if let Some(r) = o.get("recent") {
        match r.as_array() {
            Some(a) => {
                if a.len() > MAX_RECENT {
                    e.push(format!("practice: {} recent results (at most {MAX_RECENT})", a.len()));
                }
                for (i, x) in a.iter().enumerate() {
                    check_result(x, &format!("recent[{i}]"), &mut e);
                }
            }
            None => e.push("practice: recent must be a list".into()),
        }
    }
    e
}

/// Saves practice data into a song's slot by the slot mechanism of section 6 (read the
/// active state, merge, write the other slot). `edit` changes the practice value (a
/// default one when the song has none yet). If the state would not fit a slot even
/// compressed, results are dropped by the size rule of section 11 until it fits; the file
/// is never rewritten for practice data. Fails with `SlotPayloadTooLarge` only when even
/// an empty `recent` does not fit (other data fills the slot).
pub fn save_practice(path: impl AsRef<Path>, mut edit: impl FnMut(&mut Practice)) -> Result<SlotWrite> {
    let path = path.as_ref();
    let mut drop_steps = 0usize;
    loop {
        let steps = drop_steps;
        let r = update_user_state(path, |s| {
            let mut p = s.practice().unwrap_or_default();
            edit(&mut p);
            for _ in 0..steps {
                if !p.shrink_once() {
                    break;
                }
            }
            s.set_practice(&p);
        });
        match r {
            Err(e) if e.kind() == ErrorKind::SlotPayloadTooLarge => {
                drop_steps += 1;
                // 20 results x (rows + result) is at most 40 steps.
                if drop_steps > 2 * MAX_RECENT + 1 {
                    return Err(e);
                }
            }
            other => return other,
        }
    }
}

/// [`FormatError`] helper for callers that want to show why practice data was not saved.
pub fn is_too_large(e: &FormatError) -> bool {
    e.kind() == ErrorKind::SlotPayloadTooLarge
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(crate) fn result(overall: f64, rows: usize) -> PracticeResult {
        PracticeResult {
            date: "2026-11-20T19:05:00Z".into(),
            key: -3,
            key_changes: 0,
            seeks: 0,
            octave: -1,
            octave_mode: "auto".into(),
            strictness: "standard".into(),
            overall: Some(overall),
            grade: Some("B+".into()),
            graded: 212,
            total: 240,
            tendency_cents: Some(-12.3456),
            sung_share: 0.98123,
            voiced_share: 0.9149,
            completed: true,
            trust: vec!["music_in_input".into(), "removal_active".into()],
            guide: "jk-pitch pyin+notes 0.1.0".into(),
            scorer: "jk-pitch score 0.1.0".into(),
            app: "0.2.0".into(),
            lines: Some(LineBreakdown {
                basis: "lyrics".into(),
                rows: (0..rows).map(|i| LineRow(i as i64, 12.3456 + i as f64, Some(91.6), 8, Some(-4.6))).collect(),
                extra: Map::new(),
            }),
            extra: Map::new(),
        }
    }

    #[test]
    fn precision_rules_are_applied() {
        let r = result(87.4321, 2).rounded();
        assert_eq!(r.overall, Some(87.4));
        assert_eq!(r.sung_share, 0.98);
        assert_eq!(r.voiced_share, 0.91);
        assert_eq!(r.tendency_cents, Some(-12.0));
        let row = r.lines.unwrap().rows[1];
        assert_eq!((row.1, row.2, row.4), (13.3, Some(92.0), Some(-5.0)));
        let j = serde_json::to_string(&result(87.4321, 1).rounded()).unwrap();
        assert!(j.contains("\"rows\":[[0,12.3,92,8,-5]]"), "{j}");
        assert!(j.contains("\"tendency_cents\":-12,"), "{j}");
    }

    #[test]
    fn best_and_recent_rules() {
        let mut p = Practice::default();
        for i in 0..25 {
            p.add_result(result(50.0 + i as f64, 1));
        }
        assert_eq!(p.recent.len(), MAX_RECENT);
        assert_eq!(p.recent[0].overall, Some(74.0), "newest first");
        assert_eq!(p.best.as_ref().unwrap().overall, Some(74.0));
        let mut short = result(99.0, 1);
        short.sung_share = 0.5;
        p.add_result(short);
        assert_eq!(p.best.as_ref().unwrap().overall, Some(74.0), "a short take is not best");
        let mut untrusted = result(99.0, 1);
        untrusted.trust.push("untrusted".into());
        p.add_result(untrusted);
        assert_eq!(p.best.as_ref().unwrap().overall, Some(74.0), "an untrusted take is not best");
    }

    #[test]
    fn shrinking_drops_oldest_rows_then_results_never_best() {
        let mut p = Practice::default();
        for i in 0..3 {
            p.add_result(result(60.0 + i as f64, 5));
        }
        assert!(p.shrink_once());
        assert!(p.recent[2].lines.as_ref().unwrap().rows.is_empty(), "oldest rows first");
        assert!(!p.recent[1].lines.as_ref().unwrap().rows.is_empty());
        while p.shrink_once() {}
        assert!(p.recent.is_empty());
        assert!(p.best.is_some(), "best is never dropped");
    }

    #[test]
    fn user_state_keeps_unknown_keys_and_other_shapes() {
        let mut s = UserState::default();
        assert!(s.practice().is_none());
        let mut p = Practice::default();
        p.extra.insert("future".into(), Value::from(1));
        p.add_result(result(80.0, 1));
        s.set_practice(&p);
        let back = UserState::from_json(&s.to_json()).unwrap().practice().unwrap();
        assert_eq!(back.extra.get("future"), Some(&Value::from(1)));
        assert_eq!(back.recent.len(), 1);
        assert!(validate_practice(s.extra.get(PRACTICE_KEY).unwrap()).is_empty());
        // A non-object value is not parsed and stays untouched.
        s.extra.insert(PRACTICE_KEY.into(), Value::from("weird"));
        assert!(s.practice().is_none());
        assert_eq!(s.extra.get(PRACTICE_KEY), Some(&Value::from("weird")));
    }

    #[test]
    fn validation_flags_bad_types() {
        let v = serde_json::json!({"octave": {"mode": "sideways", "shift": 5}, "strictness": 3,
            "excluded": [{"start": 5, "end": 1}], "recent": [{"date": 1}]});
        let e = validate_practice(&v);
        assert_eq!(e.len(), 5, "{e:?}");
    }
}

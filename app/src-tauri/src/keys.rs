// SPDX-License-Identifier: GPL-3.0-or-later
//! Musical key names for the key control (WORK_ORDER 4.5: "A major → B major (+2)").
//!
//! Song files store tonics with sharps only (`C C# D ... B`, docs/SONG_FORMAT.md 4.1).
//! For display we use the conventional spelling, the one with the fewest accidentals
//! in the key signature, exactly as the Studio's analysis does
//! (studio/jukeoke_studio/analyse.py `spell`): Db, Eb, Ab, Bb major (F# major stays
//! sharp), Eb and Bb minor (C#, F#, G# minor stay sharp).

use serde::Serialize;

/// Tonics as stored in song files.
pub const TONICS: [&str; 12] = [
    "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B",
];

/// The arrow between the original and the shifted key (WORK_ORDER 4.5 writes "→").
pub const ARROW: &str = "→";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Major,
    Minor,
}

impl Mode {
    pub fn parse(s: &str) -> Option<Mode> {
        match s.trim().to_ascii_lowercase().as_str() {
            "major" | "maj" => Some(Mode::Major),
            "minor" | "min" => Some(Mode::Minor),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Mode::Major => "major",
            Mode::Minor => "minor",
        }
    }
}

/// A key as stored (sharp tonic index 0..12 and mode).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Key {
    pub tonic: usize,
    pub mode: Mode,
}

/// The key as the UI receives it (`SongSummary.key`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct KeyView {
    /// Sharp spelling, as stored.
    pub tonic: String,
    pub mode: String,
    /// Display name, conventional spelling: "Bb major".
    pub name: String,
}

impl Key {
    /// Parses a stored tonic (`"A#"`; flats such as `"Bb"` are accepted too) and mode.
    pub fn parse(tonic: &str, mode: &str) -> Option<Key> {
        let mode = Mode::parse(mode)?;
        let t = tonic.trim();
        let idx = TONICS.iter().position(|x| *x == t).or_else(|| {
            let flats = ["C", "Db", "D", "Eb", "E", "F", "Gb", "G", "Ab", "A", "Bb", "B"];
            flats.iter().position(|x| *x == t)
        })?;
        Some(Key { tonic: idx, mode })
    }

    /// The key `semitones` higher (mode unchanged).
    pub fn shifted(self, semitones: i32) -> Key {
        Key {
            tonic: (self.tonic as i32 + semitones).rem_euclid(12) as usize,
            mode: self.mode,
        }
    }

    /// Display tonic in the conventional spelling.
    pub fn display_tonic(self) -> &'static str {
        const FLAT: [&str; 12] = ["C", "Db", "D", "Eb", "E", "F", "Gb", "G", "Ab", "A", "Bb", "B"];
        let flat = match self.mode {
            // C#, D#, G#, A# major are written Db, Eb, Ab, Bb; F# major stays.
            Mode::Major => matches!(self.tonic, 1 | 3 | 8 | 10),
            // D#, A# minor are written Eb, Bb minor; C#, F#, G# minor stay.
            Mode::Minor => matches!(self.tonic, 3 | 10),
        };
        if flat { FLAT[self.tonic] } else { TONICS[self.tonic] }
    }

    /// "Bb major".
    pub fn name(self) -> String {
        format!("{} {}", self.display_tonic(), self.mode.as_str())
    }

    pub fn view(self) -> KeyView {
        KeyView {
            tonic: TONICS[self.tonic].to_string(),
            mode: self.mode.as_str().to_string(),
            name: self.name(),
        }
    }
}

/// The key control's label: "A major → B major (+2)", "A major (original key)", or
/// without a detected key "+2 semitones" / "original key".
pub fn key_label(key: Option<Key>, semitones: i32) -> String {
    let sign = if semitones == 0 {
        "0".to_string()
    } else {
        format!("{semitones:+}")
    };
    match key {
        None if semitones == 0 => "original key".to_string(),
        None => format!(
            "{sign} semitone{}",
            if semitones.abs() == 1 { "" } else { "s" }
        ),
        Some(k) if semitones == 0 => format!("{} (original key)", k.name()),
        Some(k) => format!("{} {ARROW} {} ({sign})", k.name(), k.shifted(semitones).name()),
    }
}

/// The key part of an export file name: "B major" or "+2 semitones"; with the shift
/// appended when there is one, e.g. "B major, +2".
pub fn export_key_text(key: Option<Key>, semitones: i32) -> String {
    match key {
        Some(k) if semitones == 0 => k.name(),
        Some(k) => format!("{}, {semitones:+}", k.shifted(semitones).name()),
        None => key_label(None, semitones),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels() {
        let a = Key::parse("A", "major");
        assert_eq!(key_label(a, 2), "A major → B major (+2)");
        assert_eq!(key_label(a, 0), "A major (original key)");
        assert_eq!(key_label(a, 1), "A major → Bb major (+1)");
        assert_eq!(key_label(a, -12), "A major → A major (-12)");
        assert_eq!(key_label(Key::parse("A#", "minor"), -1), "Bb minor → A minor (-1)");
        assert_eq!(key_label(None, 0), "original key");
        assert_eq!(key_label(None, 1), "+1 semitone");
        assert_eq!(key_label(None, -3), "-3 semitones");
    }

    #[test]
    fn conventional_spelling_matches_the_studio() {
        let names: Vec<String> = (0..12)
            .map(|t| Key { tonic: t, mode: Mode::Major }.name())
            .collect();
        assert_eq!(
            names.join(","),
            "C major,Db major,D major,Eb major,E major,F major,F# major,G major,Ab major,A major,Bb major,B major"
        );
        let names: Vec<String> = (0..12)
            .map(|t| Key { tonic: t, mode: Mode::Minor }.display_tonic().to_string())
            .collect();
        assert_eq!(names.join(","), "C,C#,D,Eb,E,F,F#,G,G#,A,Bb,B");
        assert_eq!(Key::parse("Bb", "major"), Key::parse("A#", "major"));
        assert_eq!(Key::parse("H", "major"), None);
        assert_eq!(Key::parse("C", "dorian"), None);
        assert_eq!(Key::parse("C", "minor").unwrap().view().tonic, "C");
        assert_eq!(Key::parse("C#", "major").unwrap().view().name, "Db major");
    }

    #[test]
    fn export_names() {
        let a = Key::parse("A", "major");
        assert_eq!(export_key_text(a, 0), "A major");
        assert_eq!(export_key_text(a, 2), "B major, +2");
        assert_eq!(export_key_text(None, -2), "-2 semitones");
    }
}

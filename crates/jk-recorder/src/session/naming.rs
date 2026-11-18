// SPDX-License-Identifier: GPL-3.0-or-later
//! File names for recorded songs that work on Windows (NTFS), macOS (APFS / HFS+)
//! and exFAT USB sticks: no `< > : " / \ | ? *`, no control characters, no trailing
//! dot or space, no leading dot (hidden on macOS), no reserved Windows device name,
//! and short enough that `<stick>\Recordings\<session>\<name>.wav.tmp-xxxxxxxx`
//! stays well inside the classic 260-character path limit other programs still have.

/// Longest artist part of a song file name, in characters.
pub const MAX_ARTIST_CHARS: usize = 50;
/// Longest title part of a song file name, in characters.
pub const MAX_TITLE_CHARS: usize = 70;
/// Longest file stem, in UTF-8 bytes (macOS counts bytes, Windows UTF-16 units).
pub const MAX_STEM_BYTES: usize = 180;

/// Windows device names that cannot be file names, with any extension.
const RESERVED: [&str; 22] = [
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
    "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

fn replacement(c: char) -> Option<&'static str> {
    Some(match c {
        '<' => "(",
        '>' => ")",
        ':' => " -",
        '"' => "'",
        '/' | '\\' | '|' => "-",
        '?' | '*' => "",
        c if (c as u32) < 0x20 || c == '\u{7F}' => " ",
        // Unicode controls that render invisibly or reorder text.
        '\u{200B}'..='\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}' | '\u{FEFF}' => "",
        _ => return None,
    })
}

/// Make one part of a file name safe: replace forbidden characters, collapse
/// spaces, trim, drop leading dots and trailing dots and spaces, and cut to
/// `max_chars` characters. May return an empty string.
pub fn sanitize_component(s: &str, max_chars: usize) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match replacement(c) {
            Some(r) => out.push_str(r),
            None => out.push(c),
        }
    }
    let collapsed = out.split_whitespace().collect::<Vec<_>>().join(" ");
    let cut: String = collapsed.chars().take(max_chars).collect();
    tidy_ends(&cut)
}

fn tidy_ends(s: &str) -> String {
    s.trim_start_matches(['.', ' '])
        .trim_end_matches(['.', ' '])
        .to_string()
}

/// Cut a string to at most `max_bytes` UTF-8 bytes at a character boundary.
fn cut_bytes(s: &str, max_bytes: usize) -> &str {
    if s.len() <= max_bytes {
        return s;
    }
    let mut end = max_bytes;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

/// True for a name Windows reserves for a device (`CON`, `com1.txt`, ...).
pub fn is_reserved(stem: &str) -> bool {
    let base = stem.split('.').next().unwrap_or(stem).trim_end().to_ascii_uppercase();
    // COM and LPT with superscript digits are reserved too.
    let base = base.replace(['\u{B9}', '\u{B2}', '\u{B3}'], "1");
    RESERVED.contains(&base.as_str())
}

/// A whole file stem: sanitised, never empty, never reserved, at most
/// [`MAX_STEM_BYTES`].
pub fn safe_stem(s: &str) -> String {
    let mut stem = sanitize_component(s, usize::MAX);
    stem = tidy_ends(cut_bytes(&stem, MAX_STEM_BYTES));
    if stem.is_empty() {
        stem = "Untitled".into();
    }
    if is_reserved(&stem) {
        stem = format!("_{stem}");
    }
    stem
}

/// `"<NN> <Artist> - <Title>"` (or `"<NN> <Title>"` without an artist), safe on
/// every file system the app writes to. `number` is 1-based.
pub fn song_stem(number: usize, artist: &str, title: &str) -> String {
    let artist = sanitize_component(artist, MAX_ARTIST_CHARS);
    let mut title = sanitize_component(title, MAX_TITLE_CHARS);
    if title.is_empty() {
        title = "Unknown title".into();
    }
    let stem = if artist.is_empty() {
        format!("{number:02} {title}")
    } else {
        format!("{number:02} {artist} - {title}")
    };
    safe_stem(&stem)
}

/// The folder name of a session from a local time stamp such as
/// `"2026-11-18 14-03-22"`.
pub fn session_folder_name(stamp: &str) -> String {
    safe_stem(stamp)
}

#[cfg(test)]
mod tests {
    use super::*;

    const FORBIDDEN: [char; 9] = ['<', '>', ':', '"', '/', '\\', '|', '?', '*'];

    fn assert_safe(name: &str) {
        assert!(!name.is_empty());
        assert!(!name.chars().any(|c| FORBIDDEN.contains(&c) || (c as u32) < 0x20), "{name:?}");
        assert!(!name.ends_with('.') && !name.ends_with(' '), "{name:?}");
        assert!(!name.starts_with('.') && !name.starts_with(' '), "{name:?}");
        assert!(name.len() <= MAX_STEM_BYTES, "{} bytes", name.len());
        assert!(!is_reserved(name), "{name:?}");
    }

    #[test]
    fn song_names_follow_the_pattern() {
        assert_eq!(song_stem(1, "Artist", "Title"), "01 Artist - Title");
        assert_eq!(song_stem(12, "", "Title"), "12 Title");
        assert_eq!(song_stem(3, "A", ""), "03 A - Unknown title");
        assert_eq!(song_stem(100, "A", "B"), "100 A - B");
    }

    #[test]
    fn forbidden_characters_are_replaced() {
        let s = song_stem(2, "AC/DC", "What? Is: \"this\" <a> *b* | c\\d");
        assert_eq!(s, "02 AC-DC - What Is - 'this' (a) b - c-d");
        assert_safe(&s);
        let s = song_stem(4, "Band\t\nName", "Line\u{0}One\u{7F}");
        assert_eq!(s, "04 Band Name - Line One");
        assert_safe(&s);
        let s = song_stem(5, "x\u{202E}y", "a\u{200B}b\u{FEFF}");
        assert_eq!(s, "05 xy - ab");
    }

    #[test]
    fn ends_dots_spaces_and_reserved_names() {
        assert_eq!(sanitize_component("  ...hidden. . ", 100), "hidden");
        assert_eq!(safe_stem("CON"), "_CON");
        assert_eq!(safe_stem("com1.txt"), "_com1.txt");
        assert_eq!(safe_stem("LPT\u{B9}"), "_LPT\u{B9}");
        assert!(!is_reserved("CONCERT"));
        assert!(!is_reserved("01 CON"));
        assert_eq!(safe_stem("..."), "Untitled");
        assert_eq!(safe_stem(""), "Untitled");
    }

    #[test]
    fn long_names_are_cut_on_character_boundaries() {
        let long = "ä".repeat(300);
        let s = song_stem(7, &long, &long);
        assert_safe(&s);
        // 50 + 70 two-byte letters do not fit 180 bytes: "07 " + 50 + " - " + 37.
        assert!(s.starts_with("07 ") && s.contains(" - "));
        assert_eq!(s.chars().filter(|&c| c == 'ä').count(), 87);
        let emoji = "\u{1F3A4}".repeat(200);
        let s = safe_stem(&emoji);
        assert_safe(&s);
        assert!(s.len() <= MAX_STEM_BYTES && s.len().is_multiple_of(4));
        // A cut that leaves a trailing dot or space is tidied.
        let s = sanitize_component("abc. def", 4);
        assert_eq!(s, "abc");
    }

    #[test]
    fn unicode_is_kept() {
        assert_eq!(song_stem(1, "Beyoncé", "日本語の歌"), "01 Beyoncé - 日本語の歌");
        assert_eq!(session_folder_name("2026-11-18 14-03-22"), "2026-11-18 14-03-22");
    }
}

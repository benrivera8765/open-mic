//! Table of contents (SONG_FORMAT.md section 3).

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::error::{FormatError, Result};
use crate::header::{Header, ranges_overlap};

/// One entry of the table of contents.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ChunkEntry {
    /// Unique in the file (`manifest`, `lyrics`, `cover`, `stem.<role>`, or anything a
    /// newer writer adds).
    pub id: String,
    /// Media type, for example `application/json`, `image/jpeg`, `audio/flac`.
    #[serde(rename = "type")]
    pub mime: String,
    pub offset: u64,
    pub length: u64,
    /// CRC-32 (zlib) of the chunk's bytes.
    pub crc32: u32,
    /// Keys this version does not know, kept so a rewrite does not drop them.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// The parsed table of contents.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Toc {
    pub chunks: Vec<ChunkEntry>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl Toc {
    /// Parses the TOC bytes (already CRC-checked) and checks every chunk's bounds against
    /// the header: inside the file, after both slots, not overlapping the slots or the TOC,
    /// ids unique.
    pub fn parse(bytes: &[u8], header: &Header) -> Result<Toc> {
        let toc: Toc = serde_json::from_slice(bytes)
            .map_err(|e| FormatError::damaged(format!("table of contents is not valid JSON: {e}")))?;
        let mut seen = std::collections::HashSet::new();
        let s = header.slot_size as u64;
        for c in &toc.chunks {
            if !seen.insert(c.id.as_str()) {
                return Err(FormatError::damaged(format!("chunk id '{}' appears twice", c.id)));
            }
            let end = c.offset.checked_add(c.length);
            if c.offset < crate::ALIGN || end.is_none_or(|e| e > header.file_length) {
                return Err(FormatError::damaged(format!(
                    "chunk '{}' ({} bytes at {}) lies outside the file",
                    c.id, c.length, c.offset
                )));
            }
            if ranges_overlap(c.offset, c.length, header.slot_a_offset, s)
                || ranges_overlap(c.offset, c.length, header.slot_b_offset, s)
                || ranges_overlap(c.offset, c.length, header.toc_offset, header.toc_length)
            {
                return Err(FormatError::damaged(format!(
                    "chunk '{}' overlaps a settings slot or the table of contents",
                    c.id
                )));
            }
        }
        Ok(toc)
    }

    /// The entry with this id, if any.
    pub fn get(&self, id: &str) -> Option<&ChunkEntry> {
        self.chunks.iter().find(|c| c.id == id)
    }
}

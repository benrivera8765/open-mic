//! The 4096-byte file header (SONG_FORMAT.md section 2).

use crate::error::{FormatError, Result};
use crate::{ALIGN, FORMAT_MAJOR, FORMAT_MINOR, HEADER_LENGTH, MAGIC};

/// Parsed file header. All offsets are absolute byte offsets.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Header {
    pub major: u16,
    pub minor: u16,
    /// Bytes covered by the header; the CRC is the last four of them (128 in v1.0).
    pub header_length: u32,
    pub toc_offset: u64,
    pub toc_length: u64,
    pub toc_crc32: u32,
    pub slot_size: u32,
    pub slot_a_offset: u64,
    pub slot_b_offset: u64,
    /// The total file length the writer produced (detects truncation).
    pub file_length: u64,
}

fn u16_at(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([b[o], b[o + 1]])
}
fn u32_at(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes(b[o..o + 4].try_into().unwrap())
}
fn u64_at(b: &[u8], o: usize) -> u64 {
    u64::from_le_bytes(b[o..o + 8].try_into().unwrap())
}

impl Header {
    /// A v1.0 header for a new file. Offsets are filled in by the writer.
    pub(crate) fn new_v1(slot_size: u32) -> Header {
        Header {
            major: FORMAT_MAJOR,
            minor: FORMAT_MINOR,
            header_length: HEADER_LENGTH as u32,
            toc_offset: 0,
            toc_length: 0,
            toc_crc32: 0,
            slot_size,
            slot_a_offset: ALIGN,
            slot_b_offset: ALIGN + slot_size as u64,
            file_length: 0,
        }
    }

    /// Serialises the 128 defined header bytes, including the header CRC.
    /// (Bytes 128..4096 of the file are zero.)
    pub fn encode(&self) -> [u8; HEADER_LENGTH] {
        let mut b = [0u8; HEADER_LENGTH];
        b[0..8].copy_from_slice(&MAGIC);
        b[8..10].copy_from_slice(&self.major.to_le_bytes());
        b[10..12].copy_from_slice(&self.minor.to_le_bytes());
        b[12..16].copy_from_slice(&(HEADER_LENGTH as u32).to_le_bytes());
        b[16..24].copy_from_slice(&self.toc_offset.to_le_bytes());
        b[24..32].copy_from_slice(&self.toc_length.to_le_bytes());
        b[32..36].copy_from_slice(&self.toc_crc32.to_le_bytes());
        b[36..40].copy_from_slice(&self.slot_size.to_le_bytes());
        b[40..48].copy_from_slice(&self.slot_a_offset.to_le_bytes());
        b[48..56].copy_from_slice(&self.slot_b_offset.to_le_bytes());
        b[56..64].copy_from_slice(&self.file_length.to_le_bytes());
        // 64..124 reserved, zero.
        let crc = crc32fast::hash(&b[0..HEADER_LENGTH - 4]);
        b[HEADER_LENGTH - 4..].copy_from_slice(&crc.to_le_bytes());
        b
    }

    /// Parses and checks a header, in the order SONG_FORMAT.md section 2 prescribes:
    /// magic, header CRC, major version, file length; then structural bounds.
    ///
    /// `bytes` is the start of the file (up to 4096 bytes; fewer if the file is shorter)
    /// and `actual_len` the real file length.
    ///
    /// The header CRC is taken to be the last four bytes of `header_length` (bytes
    /// 124..128 in v1.0). Reading it that way lets a future minor version grow the header
    /// without old readers calling the file damaged; see the report on the contract.
    pub fn parse(bytes: &[u8], actual_len: u64) -> Result<Header> {
        // 1. Magic. A short file whose bytes are a prefix of the magic is an incomplete
        //    copy rather than a foreign file (an empty file included).
        let n = bytes.len().min(MAGIC.len());
        if bytes[..n] != MAGIC[..n] {
            return Err(FormatError::NotASongFile);
        }
        if bytes.len() < 16 {
            return Err(FormatError::Truncated { expected: None, actual: actual_len });
        }
        let header_length = u32_at(bytes, 12) as usize;
        if !(HEADER_LENGTH..=ALIGN as usize).contains(&header_length) || header_length % 4 != 0 {
            return Err(FormatError::damaged(format!("header_length {header_length} is not valid")));
        }
        if bytes.len() < header_length {
            return Err(FormatError::Truncated { expected: None, actual: actual_len });
        }

        // 2. Header CRC.
        let stored = u32_at(bytes, header_length - 4);
        let computed = crc32fast::hash(&bytes[..header_length - 4]);
        if stored != computed {
            return Err(FormatError::damaged(format!(
                "header checksum mismatch (stored {stored:08x}, computed {computed:08x})"
            )));
        }

        // 3. Version. Any minor is accepted.
        let major = u16_at(bytes, 8);
        let minor = u16_at(bytes, 10);
        if major > FORMAT_MAJOR {
            return Err(FormatError::NewerVersion { major, minor });
        }
        if major == 0 {
            return Err(FormatError::damaged("major version 0 is not defined"));
        }

        let h = Header {
            major,
            minor,
            header_length: header_length as u32,
            toc_offset: u64_at(bytes, 16),
            toc_length: u64_at(bytes, 24),
            toc_crc32: u32_at(bytes, 32),
            slot_size: u32_at(bytes, 36),
            slot_a_offset: u64_at(bytes, 40),
            slot_b_offset: u64_at(bytes, 48),
            file_length: u64_at(bytes, 56),
        };

        // 4. Truncation.
        if actual_len < h.file_length {
            return Err(FormatError::Truncated { expected: Some(h.file_length), actual: actual_len });
        }

        h.check_structure()?;
        Ok(h)
    }

    /// Structural bounds: everything the header points at lies inside `file_length`,
    /// past the 4096-byte header block, and the two slots and the TOC do not overlap.
    fn check_structure(&self) -> Result<()> {
        let s = self.slot_size as u64;
        if s < ALIGN || s % ALIGN != 0 {
            return Err(FormatError::damaged(format!("slot_size {s} is not a multiple of 4096 (minimum 4096)")));
        }
        let within = |off: u64, len: u64| off >= ALIGN && off.checked_add(len).is_some_and(|end| end <= self.file_length);
        if !within(self.slot_a_offset, s) || !within(self.slot_b_offset, s) {
            return Err(FormatError::damaged("a settings slot lies outside the file"));
        }
        if ranges_overlap(self.slot_a_offset, s, self.slot_b_offset, s) {
            return Err(FormatError::damaged("the two settings slots overlap"));
        }
        if !within(self.toc_offset, self.toc_length) || self.toc_length == 0 {
            return Err(FormatError::damaged("the table of contents lies outside the file"));
        }
        if ranges_overlap(self.toc_offset, self.toc_length, self.slot_a_offset, s)
            || ranges_overlap(self.toc_offset, self.toc_length, self.slot_b_offset, s)
        {
            return Err(FormatError::damaged("the table of contents overlaps a settings slot"));
        }
        Ok(())
    }

    /// Offset of slot `0` (A) or `1` (B).
    pub fn slot_offset(&self, slot: usize) -> u64 {
        if slot == 0 { self.slot_a_offset } else { self.slot_b_offset }
    }
}

/// True when [a, a+alen) and [b, b+blen) share at least one byte.
pub(crate) fn ranges_overlap(a: u64, alen: u64, b: u64, blen: u64) -> bool {
    alen > 0 && blen > 0 && a < b.saturating_add(blen) && b < a.saturating_add(alen)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Header {
        let mut h = Header::new_v1(262_144);
        h.toc_offset = 4096 + 2 * 262_144 + 4096;
        h.toc_length = 100;
        h.file_length = h.toc_offset + 100;
        h
    }

    #[test]
    fn encode_parse_round_trip() {
        let h = sample();
        let mut bytes = h.encode().to_vec();
        bytes.resize(4096, 0);
        assert_eq!(Header::parse(&bytes, h.file_length).unwrap(), h);
    }

    #[test]
    fn magic_prefix_is_truncation_not_foreign() {
        let h = sample().encode();
        assert!(matches!(Header::parse(&h[..5], 5), Err(FormatError::Truncated { .. })));
        assert!(matches!(Header::parse(&[], 0), Err(FormatError::Truncated { .. })));
        assert!(matches!(Header::parse(b"PK\x03\x04zipzip", 10), Err(FormatError::NotASongFile)));
    }

    #[test]
    fn every_single_byte_flip_in_the_crc_range_is_caught() {
        let h = sample();
        let good = h.encode();
        for i in 8..HEADER_LENGTH {
            let mut b = good;
            b[i] ^= 0x01;
            let r = Header::parse(&b, h.file_length);
            assert!(r.is_err(), "flip at byte {i} was not detected");
        }
    }
}

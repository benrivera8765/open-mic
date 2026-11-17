//! Settings slots A and B (SONG_FORMAT.md section 6).
//!
//! ```text
//! offset size field
//!  0      4   magic "JKSL"
//!  4      4   slot version 1
//!  8      8   sequence number (0 = never written)
//! 16      4   payload length
//! 20      4   encoding: 0 UTF-8 JSON, 1 zlib (RFC 1950) UTF-8 JSON
//! 24      4   reserved, zero
//! 28      4   CRC-32 of bytes 0..28 followed by the payload
//! 32    ...   payload, then zero padding to slot_size
//! ```

use std::io::{Read, Write};

use crate::error::{FormatError, Result};

pub const SLOT_MAGIC: [u8; 4] = *b"JKSL";
pub const SLOT_VERSION: u32 = 1;
pub const SLOT_HEADER_LEN: usize = 32;
pub const ENCODING_JSON: u32 = 0;
pub const ENCODING_ZLIB: u32 = 1;

/// Decompressed payloads larger than this are refused (protects against a damaged or
/// hostile slot that inflates to gigabytes).
const MAX_INFLATED: u64 = 64 * 1024 * 1024;

/// What a slot holds.
#[derive(Clone, Debug, PartialEq)]
pub enum SlotState {
    /// Never written: all-zero header, or a valid header with sequence 0.
    Empty,
    /// Magic, version and CRC are right and the payload fits. `json` is the payload
    /// after decompression.
    Valid { sequence: u64, encoding: u32, json: Vec<u8> },
    /// Anything else: an interrupted write, damage, or a slot version this reader does
    /// not know. Never chosen as active.
    Invalid { reason: String },
}

impl SlotState {
    pub fn sequence(&self) -> Option<u64> {
        match self {
            SlotState::Valid { sequence, .. } => Some(*sequence),
            _ => None,
        }
    }
}

/// Decodes one slot from its raw bytes (`slot_size` of them).
pub fn decode_slot(raw: &[u8]) -> SlotState {
    if raw.len() < SLOT_HEADER_LEN {
        return SlotState::Invalid { reason: "slot is shorter than its header".into() };
    }
    let h = &raw[..SLOT_HEADER_LEN];
    if h.iter().all(|&b| b == 0) {
        return SlotState::Empty;
    }
    if h[0..4] != SLOT_MAGIC {
        return SlotState::Invalid { reason: "bad slot magic".into() };
    }
    let version = u32::from_le_bytes(h[4..8].try_into().unwrap());
    if version != SLOT_VERSION {
        return SlotState::Invalid { reason: format!("slot version {version} is not supported") };
    }
    let sequence = u64::from_le_bytes(h[8..16].try_into().unwrap());
    let len = u32::from_le_bytes(h[16..20].try_into().unwrap()) as usize;
    let encoding = u32::from_le_bytes(h[20..24].try_into().unwrap());
    let stored_crc = u32::from_le_bytes(h[28..32].try_into().unwrap());
    if len > raw.len() - SLOT_HEADER_LEN {
        return SlotState::Invalid { reason: "slot payload length exceeds the slot".into() };
    }
    let payload = &raw[SLOT_HEADER_LEN..SLOT_HEADER_LEN + len];
    let mut crc = crc32fast::Hasher::new();
    crc.update(&h[0..28]);
    crc.update(payload);
    if crc.finalize() != stored_crc {
        return SlotState::Invalid { reason: "slot checksum mismatch (interrupted or damaged write)".into() };
    }
    if sequence == 0 {
        return SlotState::Empty;
    }
    let json = match encoding {
        ENCODING_JSON => payload.to_vec(),
        ENCODING_ZLIB => {
            let mut out = Vec::new();
            let mut z = flate2::read::ZlibDecoder::new(payload).take(MAX_INFLATED + 1);
            if let Err(e) = z.read_to_end(&mut out) {
                return SlotState::Invalid { reason: format!("slot zlib payload does not inflate: {e}") };
            }
            if out.len() as u64 > MAX_INFLATED {
                return SlotState::Invalid { reason: "slot payload inflates beyond 64 MiB".into() };
            }
            out
        }
        e => return SlotState::Invalid { reason: format!("unknown slot encoding {e}") },
    };
    SlotState::Valid { sequence, encoding, json }
}

/// Builds a complete slot image (`slot_size` bytes) holding `json` with `sequence`.
/// Plain JSON is used when it fits, else zlib. Fails with
/// [`FormatError::SlotPayloadTooLarge`] when neither fits.
pub fn encode_slot(sequence: u64, json: &[u8], slot_size: u32) -> Result<Vec<u8>> {
    let capacity = slot_size as usize - SLOT_HEADER_LEN;
    let (encoding, payload): (u32, std::borrow::Cow<'_, [u8]>) = if json.len() <= capacity {
        (ENCODING_JSON, json.into())
    } else {
        let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::best());
        z.write_all(json)?;
        let compressed = z.finish()?;
        if compressed.len() > capacity {
            return Err(FormatError::SlotPayloadTooLarge { needed: compressed.len(), capacity });
        }
        (ENCODING_ZLIB, compressed.into())
    };
    let mut out = vec![0u8; slot_size as usize];
    out[0..4].copy_from_slice(&SLOT_MAGIC);
    out[4..8].copy_from_slice(&SLOT_VERSION.to_le_bytes());
    out[8..16].copy_from_slice(&sequence.to_le_bytes());
    out[16..20].copy_from_slice(&(payload.len() as u32).to_le_bytes());
    out[20..24].copy_from_slice(&encoding.to_le_bytes());
    // 24..28 reserved, zero.
    let mut crc = crc32fast::Hasher::new();
    crc.update(&out[0..28]);
    crc.update(&payload);
    out[28..32].copy_from_slice(&crc.finalize().to_le_bytes());
    out[SLOT_HEADER_LEN..SLOT_HEADER_LEN + payload.len()].copy_from_slice(&payload);
    Ok(out)
}

/// The result of looking at both slots.
#[derive(Clone, Debug)]
pub struct SlotPair {
    pub a: SlotState,
    pub b: SlotState,
    /// 0 (A) or 1 (B): the valid slot with the higher sequence whose payload is a JSON
    /// object. `None` when there are no saved settings.
    pub active: Option<usize>,
}

impl SlotPair {
    pub fn new(a: SlotState, b: SlotState) -> SlotPair {
        let usable = |s: &SlotState| match s {
            SlotState::Valid { sequence, json, .. } => {
                // A slot whose CRC is right but whose payload is not a JSON object can only
                // come from a buggy writer; fall back to the other slot rather than lose
                // everything.
                serde_json::from_slice::<serde_json::Value>(json).ok().filter(|v| v.is_object()).map(|_| *sequence)
            }
            _ => None,
        };
        let active = match (usable(&a), usable(&b)) {
            (Some(x), Some(y)) => Some(if y > x { 1 } else { 0 }),
            (Some(_), None) => Some(0),
            (None, Some(_)) => Some(1),
            (None, None) => None,
        };
        SlotPair { a, b, active }
    }

    pub fn get(&self, i: usize) -> &SlotState {
        if i == 0 { &self.a } else { &self.b }
    }

    /// Payload of the active slot.
    pub fn active_json(&self) -> Option<&[u8]> {
        match self.active.map(|i| self.get(i)) {
            Some(SlotState::Valid { json, .. }) => Some(json),
            _ => None,
        }
    }

    /// Sequence of the active slot (0 when none).
    pub fn active_sequence(&self) -> u64 {
        self.active.and_then(|i| self.get(i).sequence()).unwrap_or(0)
    }

    /// Where the next save goes: the slot that is not active (A when neither is), with
    /// sequence = highest valid sequence + 1. Never the active slot.
    pub fn next_write(&self) -> (usize, u64) {
        let target = match self.active {
            Some(0) => 1,
            _ => 0,
        };
        let highest = self.a.sequence().unwrap_or(0).max(self.b.sequence().unwrap_or(0));
        (target, highest + 1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_plain_and_zlib() {
        let small = br#"{"a":1}"#;
        let s = encode_slot(7, small, 4096).unwrap();
        assert_eq!(decode_slot(&s), SlotState::Valid { sequence: 7, encoding: 0, json: small.to_vec() });

        // Too big for plain JSON in a 4096-byte slot but compressible.
        let big = format!("{{\"x\":\"{}\"}}", "ab".repeat(5000));
        let s = encode_slot(8, big.as_bytes(), 4096).unwrap();
        match decode_slot(&s) {
            SlotState::Valid { sequence: 8, encoding: 1, json } => assert_eq!(json, big.as_bytes()),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn incompressible_payload_is_too_large() {
        // Pseudo-random hex does not compress below the slot capacity.
        let mut x: u64 = 0x1234_5678_9abc_def0;
        let mut s = String::from("{\"x\":\"");
        for _ in 0..20000 {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            s.push_str(&format!("{:x}", x & 0xf));
        }
        s.push_str("\"}");
        match encode_slot(1, s.as_bytes(), 4096) {
            Err(FormatError::SlotPayloadTooLarge { .. }) => {}
            other => panic!("expected SlotPayloadTooLarge, got {other:?}"),
        }
    }

    #[test]
    fn zero_slot_is_empty_and_garbage_is_invalid() {
        assert_eq!(decode_slot(&[0u8; 4096]), SlotState::Empty);
        let mut s = encode_slot(3, b"{}", 4096).unwrap();
        s[40] ^= 0xff; // inside the payload area, beyond the 2-byte payload: padding is not covered
        assert!(matches!(decode_slot(&s), SlotState::Valid { .. }));
        s[33] ^= 0xff; // inside the payload
        assert!(matches!(decode_slot(&s), SlotState::Invalid { .. }));
    }

    #[test]
    fn active_and_next_write() {
        let a = decode_slot(&encode_slot(4, b"{}", 4096).unwrap());
        let b = decode_slot(&encode_slot(5, b"{}", 4096).unwrap());
        let p = SlotPair::new(a.clone(), b.clone());
        assert_eq!(p.active, Some(1));
        assert_eq!(p.next_write(), (0, 6));
        let p = SlotPair::new(a, SlotState::Invalid { reason: "torn".into() });
        assert_eq!(p.active, Some(0));
        assert_eq!(p.next_write(), (1, 5));
        let p = SlotPair::new(SlotState::Empty, SlotState::Empty);
        assert_eq!(p.active, None);
        assert_eq!(p.next_write(), (0, 1));
    }
}

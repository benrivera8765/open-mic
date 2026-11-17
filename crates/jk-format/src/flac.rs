//! Minimal FLAC metadata reader (no decoding): STREAMINFO and SEEKTABLE.
//!
//! Enough to check a stem's declared format cheaply (rate, channels, bits, total
//! samples, MD5 present, seek points) without a decoder dependency.

use std::io::{self, Read};

/// The STREAMINFO block.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StreamInfo {
    pub min_block_size: u16,
    pub max_block_size: u16,
    pub min_frame_size: u32,
    pub max_frame_size: u32,
    pub sample_rate: u32,
    pub channels: u8,
    pub bits_per_sample: u8,
    /// 0 means unknown.
    pub total_samples: u64,
    /// All zero means "not computed".
    pub md5: [u8; 16],
}

/// One SEEKTABLE point. Placeholder points (sample number all ones) are dropped.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SeekPoint {
    pub sample: u64,
    /// Byte offset from the first frame header.
    pub offset: u64,
    pub frame_samples: u16,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FlacMetadata {
    pub stream_info: StreamInfo,
    /// `None` when there is no SEEKTABLE block.
    pub seek_points: Option<Vec<SeekPoint>>,
    /// Byte offset of the first audio frame from the start of the stream.
    pub first_frame_offset: u64,
    /// Metadata block types in file order (0 STREAMINFO, 1 PADDING, 3 SEEKTABLE,
    /// 4 VORBIS_COMMENT, ...).
    pub block_types: Vec<u8>,
}

fn bad(msg: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, msg.into())
}

/// Reads the `fLaC` marker and every metadata block.
pub fn read_metadata<R: Read>(mut r: R) -> io::Result<FlacMetadata> {
    let mut marker = [0u8; 4];
    r.read_exact(&mut marker)?;
    if &marker != b"fLaC" {
        return Err(bad("not a FLAC stream (no fLaC marker)"));
    }
    let mut pos = 4u64;
    let mut stream_info = None;
    let mut seek_points = None;
    let mut block_types = Vec::new();
    loop {
        let mut h = [0u8; 4];
        r.read_exact(&mut h)?;
        let last = h[0] & 0x80 != 0;
        let ty = h[0] & 0x7f;
        let len = u32::from_be_bytes([0, h[1], h[2], h[3]]) as usize;
        pos += 4 + len as u64;
        if ty == 127 {
            return Err(bad("invalid metadata block type 127"));
        }
        block_types.push(ty);
        let mut body = vec![0u8; len];
        r.read_exact(&mut body)?;
        match ty {
            0 => {
                if len < 34 {
                    return Err(bad("STREAMINFO block too short"));
                }
                if stream_info.is_some() {
                    return Err(bad("more than one STREAMINFO block"));
                }
                let b = &body;
                let packed = u64::from_be_bytes(b[10..18].try_into().unwrap());
                stream_info = Some(StreamInfo {
                    min_block_size: u16::from_be_bytes([b[0], b[1]]),
                    max_block_size: u16::from_be_bytes([b[2], b[3]]),
                    min_frame_size: u32::from_be_bytes([0, b[4], b[5], b[6]]),
                    max_frame_size: u32::from_be_bytes([0, b[7], b[8], b[9]]),
                    sample_rate: (packed >> 44) as u32,
                    channels: (((packed >> 41) & 0x7) + 1) as u8,
                    bits_per_sample: (((packed >> 36) & 0x1f) + 1) as u8,
                    total_samples: packed & 0xF_FFFF_FFFF,
                    md5: b[18..34].try_into().unwrap(),
                });
            }
            3 => {
                if len % 18 != 0 {
                    return Err(bad("SEEKTABLE length is not a multiple of 18"));
                }
                let pts = body
                    .chunks_exact(18)
                    .map(|p| SeekPoint {
                        sample: u64::from_be_bytes(p[0..8].try_into().unwrap()),
                        offset: u64::from_be_bytes(p[8..16].try_into().unwrap()),
                        frame_samples: u16::from_be_bytes([p[16], p[17]]),
                    })
                    .filter(|p| p.sample != u64::MAX)
                    .collect();
                seek_points = Some(pts);
            }
            _ => {}
        }
        if last {
            break;
        }
    }
    let stream_info = stream_info.ok_or_else(|| bad("no STREAMINFO block"))?;
    if block_types.first() != Some(&0) {
        return Err(bad("STREAMINFO is not the first metadata block"));
    }
    Ok(FlacMetadata { stream_info, seek_points, first_frame_offset: pos, block_types })
}

impl FlacMetadata {
    /// Largest gap in samples between consecutive seek points, including the gaps from
    /// the start to the first point and from the last point to the end. `None` without a
    /// seek table.
    pub fn max_seek_gap(&self) -> Option<u64> {
        let pts = self.seek_points.as_ref()?;
        let mut prev = 0u64;
        let mut gap = 0u64;
        for p in pts {
            gap = gap.max(p.sample.saturating_sub(prev));
            prev = p.sample;
        }
        gap = gap.max(self.stream_info.total_samples.saturating_sub(prev));
        Some(gap)
    }
}

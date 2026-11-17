//! # jk-format
//!
//! Reader and writer for the Jukeoke `.jksong` song file, version 1.0, as specified in
//! `docs/SONG_FORMAT.md`. No audio-device dependencies; with the default `symphonia`
//! feature, stem chunks can be handed straight to symphonia's FLAC decoder.
//!
//! ```text
//! offset 0                 file header (128 defined bytes, zero-padded to 4096)
//! offset 4096              settings slot A   (slot_size bytes)
//! offset 4096 + slot_size  settings slot B   (slot_size bytes)
//! ...                      chunks, each starting on a 4096-byte boundary
//! toc_offset               table of contents (JSON), last in the file
//! ```
//!
//! Typical use:
//!
//! ```no_run
//! # fn main() -> jk_format::Result<()> {
//! let mut song = jk_format::SongFile::open("Library/Artist - Title.jksong")?;
//! let meta = song.effective_metadata();          // override wins over the manifest
//! let settings = song.effective_settings().clone();
//! let lyrics = song.effective_lyrics()?;          // lyrics_override wins over the chunk
//! let (stem, reader) = song.stem_reader("drums")?; // Read + Seek over the FLAC chunk
//! // ... feed `reader` to a FLAC decoder, multiply samples by stem.gain / 8388608 ...
//! song.save_user_state(|s| s.settings.semitones = 2)?; // A/B slot update in place
//! # let _ = (meta, settings, lyrics, stem, reader); Ok(()) }
//! ```
//!
//! Licence: GPL-3.0-or-later.

pub mod chunk_reader;
#[cfg(feature = "symphonia")]
pub mod decode;
pub mod error;
pub mod flac;
pub mod header;
pub mod library;
pub mod lyrics;
pub mod manifest;
pub mod serde_util;
pub mod slot;
pub mod song;
pub mod time;
pub mod toc;
pub mod user_state;
pub mod verify;
pub mod writer;

pub use chunk_reader::ChunkReader;
pub use error::{ErrorKind, FormatError, Result};
pub use header::Header;
pub use library::{is_song_file_name, list_song_files, stale_temp_files};
pub use lyrics::{Grapheme, Line, Lyrics, Word};
pub use manifest::{Analysis, KeyInfo, Manifest, StemInfo, StemRole};
pub use slot::{SlotPair, SlotState};
pub use song::{SlotWrite, SongFile, update_user_state};
pub use toc::{ChunkEntry, Toc};
pub use user_state::{EffectiveMetadata, MetadataOverride, Settings, UserState};
pub use verify::{VerifyOptions, VerifyReport, verify_file};
pub use writer::{ChunkData, Rewrite, SongWriter, WrittenSong, rewrite_song};

/// `\x89JKS\r\n\x1A\n`: PNG-style magic that detects text-mode and line-ending damage.
pub const MAGIC: [u8; 8] = [0x89, b'J', b'K', b'S', 0x0D, 0x0A, 0x1A, 0x0A];
/// Highest major version this crate reads (and the one it writes).
pub const FORMAT_MAJOR: u16 = 1;
/// Minor version this crate writes. Any minor is read.
pub const FORMAT_MINOR: u16 = 0;
/// `format_version` string written into new manifests.
pub const FORMAT_VERSION: &str = "1.0";
/// Defined header bytes in v1.0 (the header block is padded to 4096).
pub const HEADER_LENGTH: usize = 128;
/// Alignment of slots, chunks and the TOC.
pub const ALIGN: u64 = 4096;
/// Slot size writers use (256 KiB).
pub const DEFAULT_SLOT_SIZE: u32 = 262_144;
/// Chunk id of the manifest.
pub const MANIFEST_CHUNK: &str = "manifest";
/// File extension, without the dot.
pub const SONG_EXTENSION: &str = "jksong";

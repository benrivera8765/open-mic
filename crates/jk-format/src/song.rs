//! Opening a song file, reading its parts, and saving the singer's settings in place.

use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use crate::chunk_reader::ChunkReader;
use crate::error::{FormatError, Result};
use crate::header::Header;
use crate::lyrics::Lyrics;
use crate::manifest::{Manifest, StemInfo};
use crate::slot::{SlotPair, SlotState, decode_slot, encode_slot};
use crate::toc::{ChunkEntry, Toc};
use crate::user_state::{EffectiveMetadata, Settings, UserState, effective_metadata};
use crate::{ALIGN, FORMAT_MAJOR};

/// JSON chunks larger than this are refused when read into memory (a damaged TOC
/// cannot make the reader allocate gigabytes).
const MAX_JSON_CHUNK: u64 = 64 * 1024 * 1024;

/// An open song file: header, table of contents, manifest and both settings slots are
/// read and checked on open; stems and other chunks are read on demand.
#[derive(Debug)]
pub struct SongFile {
    path: PathBuf,
    file: File,
    header: Header,
    toc: Toc,
    manifest: Manifest,
    slots: SlotPair,
    user_state: UserState,
    warnings: Vec<String>,
}

/// What a slot save did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SlotWrite {
    /// 0 = slot A, 1 = slot B.
    pub slot: usize,
    pub sequence: u64,
    /// Bytes of payload stored (after compression, if any).
    pub payload_bytes: usize,
}

/// Reads the header block (up to 4096 bytes) and checks it.
fn read_header(file: &mut File) -> Result<Header> {
    let actual_len = file.metadata()?.len();
    let mut buf = vec![0u8; ALIGN.min(actual_len) as usize];
    file.seek(SeekFrom::Start(0))?;
    file.read_exact(&mut buf)?;
    Header::parse(&buf, actual_len)
}

fn read_range(file: &mut File, offset: u64, len: u64) -> Result<Vec<u8>> {
    let mut buf = vec![0u8; len as usize];
    file.seek(SeekFrom::Start(offset))?;
    file.read_exact(&mut buf).map_err(|e| {
        if e.kind() == std::io::ErrorKind::UnexpectedEof {
            FormatError::damaged("file ended early")
        } else {
            e.into()
        }
    })?;
    Ok(buf)
}

fn read_toc(file: &mut File, h: &Header) -> Result<Toc> {
    if h.toc_length > MAX_JSON_CHUNK {
        return Err(FormatError::damaged("table of contents is implausibly large"));
    }
    let bytes = read_range(file, h.toc_offset, h.toc_length)?;
    let crc = crc32fast::hash(&bytes);
    if crc != h.toc_crc32 {
        return Err(FormatError::damaged(format!(
            "table of contents checksum mismatch (stored {:08x}, computed {crc:08x})",
            h.toc_crc32
        )));
    }
    Toc::parse(&bytes, h)
}

fn read_slots(file: &mut File, h: &Header) -> Result<SlotPair> {
    let a = decode_slot(&read_range(file, h.slot_a_offset, h.slot_size as u64)?);
    let b = decode_slot(&read_range(file, h.slot_b_offset, h.slot_size as u64)?);
    Ok(SlotPair::new(a, b))
}

/// Reads a chunk fully and checks its CRC.
fn read_chunk_checked(file: &mut File, c: &ChunkEntry) -> Result<Vec<u8>> {
    let bytes = read_range(file, c.offset, c.length)?;
    let crc = crc32fast::hash(&bytes);
    if crc != c.crc32 {
        return Err(FormatError::damaged(format!(
            "chunk '{}' checksum mismatch (stored {:08x}, computed {crc:08x})",
            c.id, c.crc32
        )));
    }
    Ok(bytes)
}

/// Parses the active slot's payload; falls back to the other valid slot, then to
/// defaults. Returns the state and any warnings.
fn user_state_from_slots(slots: &SlotPair) -> (UserState, Vec<String>) {
    let mut warnings = Vec::new();
    for (i, s) in [(0usize, &slots.a), (1, &slots.b)] {
        if let SlotState::Invalid { reason } = s {
            warnings.push(format!("settings slot {} ignored: {reason}", if i == 0 { 'A' } else { 'B' }));
        }
    }
    match slots.active_json().map(UserState::from_json) {
        Some(Ok(s)) => (s, warnings),
        Some(Err(e)) => {
            warnings.push(format!("saved settings unreadable, using defaults: {e}"));
            (UserState::default(), warnings)
        }
        None => (UserState::default(), warnings),
    }
}

impl SongFile {
    /// Opens and checks a song file: header (magic, CRC, version, length, bounds), TOC
    /// (CRC, chunk bounds), manifest (CRC, schema, referenced chunks present) and both
    /// slots. Stem chunk CRCs are not checked here (FLAC frames carry their own
    /// checksums); [`crate::verify::verify_file`] checks them all.
    pub fn open(path: impl AsRef<Path>) -> Result<SongFile> {
        let path = path.as_ref().to_path_buf();
        let mut file = File::open(&path)?;
        let header = read_header(&mut file)?;
        let toc = read_toc(&mut file, &header)?;
        let entry = toc.get(crate::MANIFEST_CHUNK).ok_or_else(|| FormatError::MissingChunk { id: "manifest".into() })?;
        if entry.length > MAX_JSON_CHUNK {
            return Err(FormatError::damaged("manifest is implausibly large"));
        }
        let bytes = read_chunk_checked(&mut file, entry)?;
        let manifest: Manifest = serde_json::from_slice(&bytes)
            .map_err(|e| FormatError::damaged(format!("manifest is not valid: {e}")))?;
        if manifest.format != "jksong" {
            return Err(FormatError::damaged(format!("manifest format is '{}', not 'jksong'", manifest.format)));
        }
        match manifest.format_major() {
            Some(m) if m > FORMAT_MAJOR => {
                return Err(FormatError::NewerVersion { major: m, minor: 0 });
            }
            Some(_) => {}
            None => {
                return Err(FormatError::damaged(format!(
                    "manifest format_version '{}' is not valid",
                    manifest.format_version
                )));
            }
        }
        for id in manifest.referenced_chunks() {
            if toc.get(id).is_none() {
                return Err(FormatError::MissingChunk { id: id.to_string() });
            }
        }
        let slots = read_slots(&mut file, &header)?;
        let (user_state, warnings) = user_state_from_slots(&slots);
        Ok(SongFile { path, file, header, toc, manifest, slots, user_state, warnings })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
    pub fn header(&self) -> &Header {
        &self.header
    }
    pub fn toc(&self) -> &Toc {
        &self.toc
    }
    pub fn manifest(&self) -> &Manifest {
        &self.manifest
    }
    pub fn slots(&self) -> &SlotPair {
        &self.slots
    }
    /// Non-fatal problems found on open (a damaged inactive slot, unreadable settings).
    pub fn warnings(&self) -> &[String] {
        &self.warnings
    }

    /// The saved user state from the active slot, or defaults when there is none.
    pub fn user_state(&self) -> &UserState {
        &self.user_state
    }

    /// Settings to play with: the saved ones, or defaults.
    pub fn effective_settings(&self) -> &Settings {
        &self.user_state.settings
    }

    /// Title, artist and album with `metadata_override` applied (override wins).
    pub fn effective_metadata(&self) -> EffectiveMetadata {
        effective_metadata(&self.manifest, &self.user_state)
    }

    /// Lyrics to show: `lyrics_override` from the settings when present, else the
    /// `lyrics` chunk, else `None` (instrumental).
    pub fn effective_lyrics(&self) -> Result<Option<Lyrics>> {
        if let Some(l) = &self.user_state.lyrics_override {
            return Ok(Some(l.clone()));
        }
        self.lyrics()
    }

    /// The `lyrics` chunk as written by the Studio (ignores any override).
    pub fn lyrics(&self) -> Result<Option<Lyrics>> {
        let Some(id) = &self.manifest.lyrics else { return Ok(None) };
        let bytes = self.read_chunk(id)?;
        serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|e| FormatError::damaged(format!("lyrics chunk is not valid: {e}")))
    }

    /// The cover image: (media type, bytes), or `None`.
    pub fn cover(&self) -> Result<Option<(String, Vec<u8>)>> {
        let Some(id) = &self.manifest.cover else { return Ok(None) };
        let mime = self.chunk(id).map(|c| c.mime.clone()).unwrap_or_default();
        Ok(Some((mime, self.read_chunk(id)?)))
    }

    /// TOC entry for a chunk id.
    pub fn chunk(&self, id: &str) -> Option<&ChunkEntry> {
        self.toc.get(id)
    }

    /// Reads a whole chunk into memory and checks its CRC. Meant for small chunks
    /// (JSON, cover); stream stems with [`SongFile::stem_reader`].
    pub fn read_chunk(&self, id: &str) -> Result<Vec<u8>> {
        let c = self.chunk(id).ok_or_else(|| FormatError::MissingChunk { id: id.into() })?;
        if c.length > MAX_JSON_CHUNK {
            return Err(FormatError::invalid(format!("chunk '{id}' is too large to read into memory; stream it")));
        }
        // Positional read: safe to call from several threads on the same SongFile.
        let r = ChunkReader::new(self.file.try_clone()?, c.offset, c.length);
        let mut bytes = vec![0u8; c.length as usize];
        r.read_exact_at(&mut bytes, 0)?;
        let crc = crc32fast::hash(&bytes);
        if crc != c.crc32 {
            return Err(FormatError::damaged(format!(
                "chunk '{}' checksum mismatch (stored {:08x}, computed {crc:08x})",
                c.id, c.crc32
            )));
        }
        Ok(bytes)
    }

    /// A `Read + Seek` view of a chunk with its own file handle.
    pub fn chunk_reader(&self, id: &str) -> Result<ChunkReader> {
        let c = self.chunk(id).ok_or_else(|| FormatError::MissingChunk { id: id.into() })?;
        Ok(ChunkReader::new(self.file.try_clone()?, c.offset, c.length))
    }

    /// The stem with this role and a reader over its FLAC stream.
    pub fn stem_reader(&self, role: &str) -> Result<(StemInfo, ChunkReader)> {
        let stem = self
            .manifest
            .stem(role)
            .ok_or_else(|| FormatError::MissingChunk { id: format!("stem.{role}") })?;
        Ok((stem.clone(), self.chunk_reader(&stem.chunk)?))
    }

    /// Computes every chunk's CRC (streams through the stems). Returns the ids that
    /// fail.
    pub fn check_all_chunk_crcs(&self) -> Result<Vec<String>> {
        let mut bad = Vec::new();
        let mut buf = vec![0u8; 1 << 20];
        for c in &self.toc.chunks {
            let mut r = ChunkReader::new(self.file.try_clone()?, c.offset, c.length);
            let mut h = crc32fast::Hasher::new();
            loop {
                let n = r.read(&mut buf)?;
                if n == 0 {
                    break;
                }
                h.update(&buf[..n]);
            }
            if h.finalize() != c.crc32 {
                bad.push(c.id.clone());
            }
        }
        Ok(bad)
    }

    /// Read-merge-write of the singer's state into the inactive slot (section 6).
    ///
    /// The active slot is re-read from disk (not taken from what was cached at open),
    /// `edit` changes the typed state (unknown keys are kept), and the result goes into
    /// the other slot with sequence + 1, followed by `sync_data`. The active slot is
    /// never touched, so an interrupted save leaves the previous settings intact.
    ///
    /// Fails with [`FormatError::SlotPayloadTooLarge`] when the state does not fit in a
    /// slot even compressed; the caller then uses [`crate::writer::rewrite_song`].
    pub fn save_user_state(&mut self, edit: impl FnOnce(&mut UserState)) -> Result<SlotWrite> {
        let mut f = OpenOptions::new().read(true).write(true).open(&self.path)?;
        // The path must still be the file we opened: same header (TOC position + CRC).
        let h = read_header(&mut f)?;
        if h != self.header {
            return Err(FormatError::ChangedOnDisk);
        }
        let (w, state, slots) = update_slots(&mut f, &h, edit)?;
        self.user_state = state;
        self.slots = slots;
        Ok(w)
    }
}

/// The slot update shared by [`SongFile::save_user_state`] and [`update_user_state`].
fn update_slots(
    f: &mut File,
    h: &Header,
    edit: impl FnOnce(&mut UserState),
) -> Result<(SlotWrite, UserState, SlotPair)> {
    let slots = read_slots(f, h)?;
    let (mut state, _) = user_state_from_slots(&slots);
    edit(&mut state);
    state.updated_utc = Some(crate::time::utc_now_iso8601());
    let json = state.to_json();
    let (target, sequence) = slots.next_write();
    let image = encode_slot(sequence, &json, h.slot_size)?;
    let payload_bytes = u32::from_le_bytes(image[16..20].try_into().unwrap()) as usize;
    f.seek(SeekFrom::Start(h.slot_offset(target)))?;
    f.write_all(&image)?;
    f.sync_data()?;
    let new_slots = read_slots(f, h)?;
    if new_slots.active != Some(target) {
        return Err(FormatError::damaged("settings slot did not read back after writing"));
    }
    Ok((SlotWrite { slot: target, sequence, payload_bytes }, state, new_slots))
}

/// Saves the singer's state into a song file by path, without keeping it open:
/// read-merge-write into the inactive slot (see [`SongFile::save_user_state`]).
pub fn update_user_state(path: impl AsRef<Path>, edit: impl FnOnce(&mut UserState)) -> Result<SlotWrite> {
    let mut f = OpenOptions::new().read(true).write(true).open(path.as_ref())?;
    let h = read_header(&mut f)?;
    // Validate the rest of the file's structure before writing into it.
    read_toc(&mut f, &h)?;
    update_slots(&mut f, &h, edit).map(|(w, _, _)| w)
}

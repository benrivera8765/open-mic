//! Writing a whole song file (SONG_FORMAT.md section 7), atomically.
//!
//! Order on disk: header placeholder, slot A (initial user state, sequence 1), slot B
//! (empty), chunks (each 4096-aligned, zero padding between; the manifest first), the
//! TOC, then the real header. The file is written as `<name>.tmp-<random>` next to the
//! destination, flushed to disk, and renamed over the destination, so a crash or a
//! pulled USB stick leaves either the old file or the new one.

use std::fs::{File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::chunk_reader::ChunkReader;
use crate::error::{FormatError, Result};
use crate::header::Header;
use crate::manifest::Manifest;
use crate::slot::{SLOT_HEADER_LEN, encode_slot};
use crate::song::SongFile;
use crate::toc::{ChunkEntry, Toc};
use crate::user_state::UserState;
use crate::{ALIGN, DEFAULT_SLOT_SIZE, MANIFEST_CHUNK};

/// Where a chunk's bytes come from.
pub enum ChunkData {
    /// Bytes in memory.
    Bytes(Vec<u8>),
    /// `length` bytes at `offset` of a file on disk, copied by streaming. When
    /// `expected_crc32` is set the copy is checked against it, so a damaged source is
    /// never re-stamped with a fresh, valid checksum.
    FileRange { path: PathBuf, offset: u64, length: u64, expected_crc32: Option<u32> },
    /// A whole file on disk (for example a FLAC stream the caller encoded).
    File(PathBuf),
}

struct PendingChunk {
    id: String,
    mime: String,
    data: ChunkData,
    extra: serde_json::Map<String, serde_json::Value>,
}

/// Summary of a written file.
#[derive(Clone, Debug)]
pub struct WrittenSong {
    pub path: PathBuf,
    pub header: Header,
    pub toc: Toc,
}

/// Builds a complete song file. The manifest is stored as the `manifest` chunk; every
/// chunk the manifest refers to must be added.
pub struct SongWriter {
    manifest: Manifest,
    chunks: Vec<PendingChunk>,
    user_state: UserState,
    slot_size: u32,
    /// Unknown top-level TOC keys to write back (kept by `rewrite_song`).
    toc_extra: serde_json::Map<String, serde_json::Value>,
}

impl SongWriter {
    pub fn new(manifest: Manifest) -> SongWriter {
        SongWriter {
            manifest,
            chunks: Vec::new(),
            user_state: UserState::default(),
            slot_size: DEFAULT_SLOT_SIZE,
            toc_extra: Default::default(),
        }
    }

    /// The initial user state written into slot A (default: default settings).
    pub fn user_state(mut self, state: UserState) -> Self {
        self.user_state = state;
        self
    }

    /// Slot size in bytes (multiple of 4096, at least 4096). The writer grows it when
    /// the initial user state would not fit.
    pub fn slot_size(mut self, bytes: u32) -> Self {
        self.slot_size = bytes;
        self
    }

    /// Adds a chunk. Chunks are written in the order added, after the manifest.
    pub fn add_chunk(&mut self, id: impl Into<String>, mime: impl Into<String>, data: ChunkData) -> Result<()> {
        let id = id.into();
        if id == MANIFEST_CHUNK {
            return Err(FormatError::invalid("the manifest chunk is written by the writer itself"));
        }
        if id.is_empty() || self.chunks.iter().any(|c| c.id == id) {
            return Err(FormatError::invalid(format!("chunk id '{id}' is empty or already used")));
        }
        self.chunks.push(PendingChunk { id, mime: mime.into(), data, extra: Default::default() });
        Ok(())
    }

    /// Adds a JSON chunk (`application/json`).
    pub fn add_json_chunk(&mut self, id: impl Into<String>, value: &impl Serialize) -> Result<()> {
        let bytes = serde_json::to_vec(value).map_err(|e| FormatError::invalid(e.to_string()))?;
        self.add_chunk(id, "application/json", ChunkData::Bytes(bytes))
    }

    fn check(&mut self) -> Result<()> {
        if self.slot_size < ALIGN as u32 || self.slot_size % ALIGN as u32 != 0 {
            return Err(FormatError::invalid("slot_size must be a multiple of 4096, at least 4096"));
        }
        for id in self.manifest.referenced_chunks() {
            if !self.chunks.iter().any(|c| c.id == id) {
                return Err(FormatError::invalid(format!("the manifest refers to chunk '{id}' which was not added")));
            }
        }
        if !(self.manifest.channels == 1 || self.manifest.channels == 2) || self.manifest.sample_rate == 0 {
            return Err(FormatError::invalid("channels must be 1 or 2 and sample_rate > 0"));
        }
        Ok(())
    }

    /// Writes the file atomically to `dest` (temp file in the same folder, `sync_all`,
    /// rename). Returns the header and TOC as written.
    pub fn write_atomic(mut self, dest: impl AsRef<Path>) -> Result<WrittenSong> {
        self.check()?;
        let dest = dest.as_ref();
        let tmp = temp_path_for(dest)?;
        let result = self.write_to(&tmp);
        let (header, toc) = match result {
            Ok(x) => x,
            Err(e) => {
                let _ = std::fs::remove_file(&tmp);
                return Err(e);
            }
        };
        if let Err(e) = std::fs::rename(&tmp, dest) {
            let _ = std::fs::remove_file(&tmp);
            return Err(e.into());
        }
        sync_parent_dir(dest);
        Ok(WrittenSong { path: dest.to_path_buf(), header, toc })
    }

    fn write_to(self, tmp: &Path) -> Result<(Header, Toc)> {
        // Initial user state in slot A with sequence 1; grow the slots if needed.
        let state_json = self.user_state.to_json();
        let mut slot_size = self.slot_size;
        let slot_a = loop {
            match encode_slot(1, &state_json, slot_size) {
                Ok(s) => break s,
                Err(FormatError::SlotPayloadTooLarge { needed, .. }) => {
                    slot_size = grown_slot_size(needed);
                }
                Err(e) => return Err(e),
            }
        };

        let mut f = OpenOptions::new().write(true).create_new(true).open(tmp)?;
        let mut header = Header::new_v1(slot_size);
        let mut w = CountingWriter { inner: &mut f, pos: 0 };

        // Header placeholder (zeros), slot A, slot B (empty = zeros).
        w.write_all(&vec![0u8; ALIGN as usize])?;
        w.write_all(&slot_a)?;
        w.write_zeros(slot_size as u64)?;

        let mut entries = Vec::with_capacity(self.chunks.len() + 1);
        let manifest_bytes = serde_json::to_vec(&self.manifest).map_err(|e| FormatError::invalid(e.to_string()))?;
        let mut all = Vec::with_capacity(self.chunks.len() + 1);
        all.push(PendingChunk {
            id: MANIFEST_CHUNK.into(),
            mime: "application/json".into(),
            data: ChunkData::Bytes(manifest_bytes),
            extra: Default::default(),
        });
        all.extend(self.chunks);

        let mut buf = vec![0u8; 1 << 20];
        for c in all {
            w.pad_to(ALIGN)?;
            let offset = w.pos;
            let (length, crc32) = match c.data {
                ChunkData::Bytes(b) => {
                    w.write_all(&b)?;
                    (b.len() as u64, crc32fast::hash(&b))
                }
                ChunkData::File(path) => {
                    let src = File::open(&path)?;
                    let len = src.metadata()?.len();
                    copy_range(src, 0, len, &mut w, &mut buf)?
                }
                ChunkData::FileRange { path, offset: src_off, length, expected_crc32 } => {
                    let src = File::open(&path)?;
                    let (len, crc) = copy_range(src, src_off, length, &mut w, &mut buf)?;
                    if let Some(want) = expected_crc32 {
                        if want != crc {
                            return Err(FormatError::damaged(format!(
                                "chunk '{}' of the source file is damaged (checksum mismatch); not copied",
                                c.id
                            )));
                        }
                    }
                    (len, crc)
                }
            };
            entries.push(ChunkEntry { id: c.id, mime: c.mime, offset, length, crc32, extra: c.extra });
        }

        // TOC, 4096-aligned like the chunks, last in the file.
        let toc = Toc { chunks: entries, extra: self.toc_extra };
        let toc_bytes = serde_json::to_vec(&toc).map_err(|e| FormatError::invalid(e.to_string()))?;
        w.pad_to(ALIGN)?;
        header.toc_offset = w.pos;
        header.toc_length = toc_bytes.len() as u64;
        header.toc_crc32 = crc32fast::hash(&toc_bytes);
        w.write_all(&toc_bytes)?;
        header.file_length = w.pos;

        // Final header, then flush everything to the device.
        f.seek(SeekFrom::Start(0))?;
        f.write_all(&header.encode())?;
        f.sync_all()?;
        drop(f);
        Ok((header, toc))
    }
}

/// A slot size big enough for `needed` payload bytes with room to grow: twice the
/// need, rounded up to 4096, at least the default.
fn grown_slot_size(needed: usize) -> u32 {
    let want = (needed + SLOT_HEADER_LEN) as u64 * 2;
    let rounded = want.div_ceil(ALIGN) * ALIGN;
    rounded.max(DEFAULT_SLOT_SIZE as u64).min(u32::MAX as u64 / 2) as u32
}

/// Copies `len` bytes at `off` of `src` to `w`, returning (length, CRC-32).
fn copy_range(src: File, off: u64, len: u64, w: &mut impl Write, buf: &mut [u8]) -> Result<(u64, u32)> {
    let mut r = ChunkReader::new(src, off, len);
    let mut h = crc32fast::Hasher::new();
    let mut done = 0u64;
    loop {
        let n = r.read(buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
        w.write_all(&buf[..n])?;
        done += n as u64;
    }
    if done != len {
        return Err(FormatError::Io(io::Error::new(io::ErrorKind::UnexpectedEof, "source chunk ended early")));
    }
    Ok((len, h.finalize()))
}

struct CountingWriter<'a> {
    inner: &'a mut File,
    pos: u64,
}

impl CountingWriter<'_> {
    fn write_zeros(&mut self, mut n: u64) -> io::Result<()> {
        let zeros = [0u8; 8192];
        while n > 0 {
            let k = n.min(zeros.len() as u64) as usize;
            self.write_all(&zeros[..k])?;
            n -= k as u64;
        }
        Ok(())
    }

    fn pad_to(&mut self, align: u64) -> io::Result<()> {
        let pad = (align - self.pos % align) % align;
        self.write_zeros(pad)
    }
}

impl Write for CountingWriter<'_> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let n = self.inner.write(buf)?;
        self.pos += n as u64;
        Ok(n)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

/// `<dir>/<name>.tmp-<16 hex digits>` next to `dest`.
pub fn temp_path_for(dest: &Path) -> Result<PathBuf> {
    let name = dest
        .file_name()
        .ok_or_else(|| FormatError::invalid("destination has no file name"))?
        .to_string_lossy()
        .into_owned();
    Ok(dest.with_file_name(format!("{name}.tmp-{:016x}", random_u64())))
}

/// 64 random-enough bits from std only: `RandomState` is seeded from the OS RNG; mixed
/// with the time, the process id and a counter.
fn random_u64() -> u64 {
    use std::collections::hash_map::RandomState;
    use std::hash::{BuildHasher, Hasher};
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let mut h = RandomState::new().build_hasher();
    h.write_u64(COUNTER.fetch_add(1, Ordering::Relaxed));
    h.write_u32(std::process::id());
    if let Ok(d) = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
        h.write_u128(d.as_nanos());
    }
    h.finish()
}

/// [`sync_parent_dir`] for the guide rewrite (crate::guide_write).
pub(crate) fn sync_parent_dir_pub(path: &Path) {
    sync_parent_dir(path)
}

/// After a rename, flush the directory entry on Unix (so the rename itself survives a
/// power cut). Windows has no portable equivalent; NTFS/exFAT metadata is flushed by
/// the file system.
fn sync_parent_dir(path: &Path) {
    #[cfg(unix)]
    if let Some(dir) = path.parent() {
        let dir = if dir.as_os_str().is_empty() { Path::new(".") } else { dir };
        if let Ok(d) = File::open(dir) {
            let _ = d.sync_all();
        }
    }
    #[cfg(not(unix))]
    let _ = path;
}

/// What to change when rewriting an existing file.
#[derive(Default)]
pub struct Rewrite {
    /// Replace the manifest (unknown keys in the new value are written as given).
    pub manifest: Option<Manifest>,
    /// Replace the user state; `None` keeps the active slot's state.
    pub user_state: Option<UserState>,
    /// Add or replace chunks (by id). Chunks not named here are copied unchanged,
    /// including chunks this version does not understand.
    pub chunks: Vec<(String, String, ChunkData)>,
    /// Drop these chunk ids.
    pub remove: Vec<String>,
    /// Slot size for the new file; `None` keeps the old one (grown if the state needs it).
    pub slot_size: Option<u32>,
}

/// Rewrites a whole song file in place (atomically): copies every chunk it is not told
/// to change byte for byte (each checked against its CRC while copying), keeps chunk
/// order, and stores the user state in slot A of the new file. This is the fallback
/// when [`crate::SongFile::save_user_state`] reports `SlotPayloadTooLarge`: the new
/// file gets slots large enough for the state.
///
/// All handles to the old file must be closed by the caller before calling this on
/// Windows when the file lives on exFAT/FAT (those file systems cannot replace a file
/// that is open).
pub fn rewrite_song(path: impl AsRef<Path>, changes: Rewrite) -> Result<WrittenSong> {
    let path = path.as_ref();
    // Gather everything needed, then close the source before the rename.
    let (manifest, user_state, old_slot_size, old_toc) = {
        let song = SongFile::open(path)?;
        (song.manifest().clone(), song.user_state().clone(), song.header().slot_size, song.toc().clone())
    };
    let old_chunks = old_toc.chunks;
    let manifest = changes.manifest.unwrap_or(manifest);
    let user_state = changes.user_state.unwrap_or(user_state);
    let mut w = SongWriter::new(manifest).user_state(user_state).slot_size(changes.slot_size.unwrap_or(old_slot_size));
    w.toc_extra = old_toc.extra;
    let mut replacements = changes.chunks;
    for c in old_chunks {
        if c.id == MANIFEST_CHUNK || changes.remove.contains(&c.id) {
            continue;
        }
        if let Some(i) = replacements.iter().position(|(id, _, _)| *id == c.id) {
            let (id, mime, data) = replacements.remove(i);
            w.add_chunk(id, mime, data)?;
        } else {
            let id = c.id.clone();
            w.add_chunk(
                c.id,
                c.mime,
                ChunkData::FileRange { path: path.to_path_buf(), offset: c.offset, length: c.length, expected_crc32: Some(c.crc32) },
            )?;
            // Keep unknown keys of the TOC entry too.
            if let Some(p) = w.chunks.iter_mut().find(|p| p.id == id) {
                p.extra = c.extra;
            }
        }
    }
    for (id, mime, data) in replacements {
        w.add_chunk(id, mime, data)?;
    }
    w.write_atomic(path)
}

// SPDX-License-Identifier: GPL-3.0-or-later
//! Adding or replacing the melody guide in an existing song file: the raw-copy rewrite
//! of SONG_FORMAT.md section 7.1.
//!
//! Unlike [`crate::writer::rewrite_song`] (which re-encodes the manifest and writes the
//! user state into slot A, its job being to grow the slots), this copies **everything
//! raw**: both settings slot regions byte for byte (sequence numbers, CRCs, padding, even
//! an invalid or never-written slot), and every chunk in TOC order with its CRC checked
//! while copying, the manifest included. Only the header, the TOC and the two guide
//! chunks change. So the stems, lyrics, cover and saved settings are unchanged to the
//! byte (WO2 4.3), which `tests/format.rs` and `tests/conformance` check by SHA-256.
//!
//! The work is split in two so the app can defer the swap while the song plays
//! (docs/APP_API.md 12.3):
//!
//! 1. [`prepare_guide_rewrite`] (section 7.1 step 2): writes `<name>.jksong.tmp-<random>`
//!    next to the song. The song may keep playing: the original is only read.
//! 2. [`PreparedGuide::finish`] (steps 3-5): with the song released by the caller (on
//!    Windows and exFAT an open file cannot be replaced), re-reads the original's header
//!    and TOC, checks that nothing but the guide differs, copies both slot regions again
//!    (so a settings save made meanwhile is not lost), checks the temporary file, and
//!    renames it over the original.
//!
//! A crash or a pulled stick before the rename leaves the original untouched and a stray
//! `*.tmp-*` file (readers ignore it; the app deletes leftovers older than an hour);
//! after the rename the new file is complete.

use std::fs::{File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use crate::chunk_reader::ChunkReader;
use crate::error::{FormatError, Result};
use crate::guide::{CURVE_CHUNK, CURVE_MIME, GUIDE_CHUNK, GUIDE_MIME};
use crate::header::Header;
use crate::toc::{ChunkEntry, Toc};
use crate::writer::{WrittenSong, temp_path_for};
use crate::{ALIGN, HEADER_LENGTH};

/// The two chunks to add or replace.
#[derive(Clone, Debug)]
pub struct GuideChunks {
    /// The `guide` chunk (UTF-8 JSON).
    pub guide_json: Vec<u8>,
    /// The `guide.curve` chunk (section 10.3).
    pub curve: Vec<u8>,
}

/// Simulated interruptions, for tests of section 7.1 step 6 ("a pulled stick leaves the
/// old file or the new one"). At each point the rewrite stops as a crash would: the
/// temporary file is left as it is (or cut short), nothing is cleaned up.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FailPoint {
    /// After the header placeholder.
    AfterPlaceholder,
    /// After both slot regions.
    AfterSlots,
    /// In the middle of copying the chunks (after the first one).
    MidChunks,
    /// After every chunk, before the TOC.
    BeforeToc,
    /// After the TOC, before the final header.
    BeforeHeader,
    /// After the temporary file is complete, before step 3 (the second slot copy).
    BeforeSecondSlotCopy,
    /// After step 4's checks, before the rename.
    BeforeRename,
    /// The temporary file cut to half its length (power lost while writing), before the
    /// rename.
    TruncatedTemp,
}

/// Cancellation, progress and (tests only) a simulated interruption.
#[derive(Default)]
pub struct RewriteControl<'a> {
    /// Checked between copy blocks; a cancelled rewrite deletes its temporary file and
    /// returns an error for which [`is_cancelled`] is true.
    pub cancel: Option<&'a AtomicBool>,
    /// Called with (bytes written, bytes to write) while the temporary file is written.
    pub progress: Option<&'a mut dyn FnMut(u64, u64)>,
    /// Tests: stop at this point as a crash would.
    pub fail_at: Option<FailPoint>,
}

/// The error a cancelled rewrite returns.
pub fn cancelled_error() -> FormatError {
    FormatError::Io(io::Error::new(io::ErrorKind::Interrupted, "cancelled"))
}

/// True when `e` is the error of a cancelled rewrite.
pub fn is_cancelled(e: &FormatError) -> bool {
    matches!(e, FormatError::Io(x) if x.kind() == io::ErrorKind::Interrupted && x.to_string() == "cancelled")
}

/// The error a simulated interruption returns (tests).
fn simulated(p: FailPoint) -> FormatError {
    FormatError::Io(io::Error::other(format!("simulated interruption at {p:?}")))
}

/// A temporary file holding the song with its new guide, ready to replace the original.
/// Dropping it without [`PreparedGuide::finish`] deletes the temporary file.
#[derive(Debug)]
pub struct PreparedGuide {
    /// The song file to replace.
    pub original: PathBuf,
    /// `<name>.jksong.tmp-<random>` next to it.
    pub tmp: PathBuf,
    /// The original's header when the temporary file was written.
    pub original_header: Header,
    /// Header and TOC of the temporary file.
    pub header: Header,
    pub toc: Toc,
    /// CRC-32 of the new guide and curve chunks.
    pub guide_crc32: u32,
    pub curve_crc32: u32,
    /// Bytes of the new file.
    pub file_length: u64,
    /// Set once renamed (or deliberately left for a test).
    keep: bool,
}

impl Drop for PreparedGuide {
    fn drop(&mut self) {
        if !self.keep {
            let _ = std::fs::remove_file(&self.tmp);
        }
    }
}

/// Reads and checks the header and TOC of a song file (no manifest needed: a raw copy
/// does not interpret the chunks).
fn read_header_and_toc(f: &mut File) -> Result<(Header, Toc)> {
    let actual = f.metadata()?.len();
    let mut buf = vec![0u8; (ALIGN.min(actual)) as usize];
    f.seek(SeekFrom::Start(0))?;
    f.read_exact(&mut buf)?;
    let h = Header::parse(&buf, actual)?;
    if h.toc_length > 64 * 1024 * 1024 {
        return Err(FormatError::damaged("table of contents is implausibly large"));
    }
    let mut t = vec![0u8; h.toc_length as usize];
    f.seek(SeekFrom::Start(h.toc_offset))?;
    f.read_exact(&mut t)?;
    let crc = crc32fast::hash(&t);
    if crc != h.toc_crc32 {
        return Err(FormatError::damaged(format!(
            "table of contents checksum mismatch (stored {:08x}, computed {crc:08x})",
            h.toc_crc32
        )));
    }
    let toc = Toc::parse(&t, &h)?;
    Ok((h, toc))
}

fn read_exact_at(f: &mut File, off: u64, len: u64) -> Result<Vec<u8>> {
    let mut b = vec![0u8; len as usize];
    f.seek(SeekFrom::Start(off))?;
    f.read_exact(&mut b)?;
    Ok(b)
}

/// Both slot regions of a file, raw (`slot_size` bytes each).
fn read_slot_regions(f: &mut File, h: &Header) -> Result<(Vec<u8>, Vec<u8>)> {
    let a = read_exact_at(f, h.slot_a_offset, h.slot_size as u64)?;
    let b = read_exact_at(f, h.slot_b_offset, h.slot_size as u64)?;
    Ok((a, b))
}

/// The chunks other than the guide's, as (id, type, length, crc32, unknown keys).
fn kept_entries(t: &Toc) -> Vec<&ChunkEntry> {
    t.chunks.iter().filter(|c| c.id != GUIDE_CHUNK && c.id != CURVE_CHUNK).collect()
}

fn same_kept_chunks(a: &Toc, b: &Toc) -> bool {
    let (x, y) = (kept_entries(a), kept_entries(b));
    x.len() == y.len()
        && x.iter().zip(&y).all(|(p, q)| p.id == q.id && p.mime == q.mime && p.length == q.length && p.crc32 == q.crc32)
}

struct Out<'a, 'b> {
    f: &'a mut File,
    pos: u64,
    total: u64,
    ctl: &'a mut RewriteControl<'b>,
}

impl Out<'_, '_> {
    fn write(&mut self, b: &[u8]) -> Result<()> {
        if self.ctl.cancel.is_some_and(|c| c.load(Ordering::Relaxed)) {
            return Err(cancelled_error());
        }
        self.f.write_all(b)?;
        self.pos += b.len() as u64;
        let (pos, total) = (self.pos, self.total);
        if let Some(p) = self.ctl.progress.as_mut() {
            p(pos, total);
        }
        Ok(())
    }

    fn zeros(&mut self, mut n: u64) -> Result<()> {
        let z = [0u8; 8192];
        while n > 0 {
            let k = n.min(z.len() as u64) as usize;
            self.write(&z[..k])?;
            n -= k as u64;
        }
        Ok(())
    }

    fn align(&mut self) -> Result<()> {
        let pad = (ALIGN - self.pos % ALIGN) % ALIGN;
        self.zeros(pad)
    }

    /// Copies `len` bytes at `off` of `src`, checking them against `crc` (a mismatch
    /// stops the rewrite: the original is damaged and is left alone).
    fn copy_checked(&mut self, src: &File, id: &str, off: u64, len: u64, crc: u32, buf: &mut [u8]) -> Result<()> {
        let mut r = ChunkReader::new(src.try_clone()?, off, len);
        let mut h = crc32fast::Hasher::new();
        let mut done = 0u64;
        loop {
            let n = r.read(buf)?;
            if n == 0 {
                break;
            }
            h.update(&buf[..n]);
            self.write(&buf[..n])?;
            done += n as u64;
        }
        if done != len {
            return Err(FormatError::damaged(format!("chunk '{id}' ended early")));
        }
        let got = h.finalize();
        if got != crc {
            return Err(FormatError::damaged(format!(
                "chunk '{id}' checksum mismatch while copying (stored {crc:08x}, computed {got:08x}); the song was left as it was"
            )));
        }
        Ok(())
    }
}

/// Section 7.1 step 2: writes the temporary file (header placeholder, both slot regions
/// raw, every chunk raw in TOC order with an existing guide replaced in place or a new one
/// appended, the TOC, the final header), flushed to disk. The original is only read.
///
/// On an error the temporary file is deleted (except for a simulated interruption).
pub fn prepare_guide_rewrite(path: impl AsRef<Path>, chunks: &GuideChunks, ctl: &mut RewriteControl<'_>) -> Result<PreparedGuide> {
    let path = path.as_ref();
    let mut src = File::open(path)?;
    let (oh, otoc) = read_header_and_toc(&mut src)?;
    if oh.header_length as usize != HEADER_LENGTH {
        // A newer minor version grew the header; a 1.1 writer cannot copy what it means.
        return Err(FormatError::invalid(format!(
            "header_length {} is newer than this writer (128); not rewritten",
            oh.header_length
        )));
    }
    let tmp = temp_path_for(path)?;
    let mut f = OpenOptions::new().write(true).create_new(true).open(&tmp)?;
    let mut prepared = PreparedGuide {
        original: path.to_path_buf(),
        tmp: tmp.clone(),
        original_header: oh.clone(),
        header: oh.clone(),
        toc: Toc::default(),
        guide_crc32: crc32fast::hash(&chunks.guide_json),
        curve_crc32: crc32fast::hash(&chunks.curve),
        file_length: 0,
        keep: false,
    };
    let fail_at = ctl.fail_at;
    let result = write_temp(&mut src, &mut f, &oh, &otoc, chunks, ctl, &mut prepared);
    drop(f);
    match result {
        Ok(()) => Ok(prepared),
        Err(e) => {
            if fail_at.is_some() && e.to_string().contains("simulated interruption") {
                prepared.keep = true; // a crash leaves the temporary file behind
            }
            Err(e)
        }
    }
}

fn write_temp(
    src: &mut File,
    f: &mut File,
    oh: &Header,
    otoc: &Toc,
    chunks: &GuideChunks,
    ctl: &mut RewriteControl<'_>,
    p: &mut PreparedGuide,
) -> Result<()> {
    let s = oh.slot_size as u64;
    let has_guide = otoc.get(GUIDE_CHUNK).is_some();
    let has_curve = otoc.get(CURVE_CHUNK).is_some();
    let total = ALIGN
        + 2 * s
        + otoc
            .chunks
            .iter()
            .filter(|c| c.id != GUIDE_CHUNK && c.id != CURVE_CHUNK)
            .map(|c| c.length.div_ceil(ALIGN) * ALIGN)
            .sum::<u64>()
        + (chunks.guide_json.len() + chunks.curve.len()) as u64;
    let fail_at = ctl.fail_at;
    let fail = |at: FailPoint| -> Result<()> { if fail_at == Some(at) { Err(simulated(at)) } else { Ok(()) } };
    let (slot_a, slot_b) = read_slot_regions(src, oh)?;
    let mut out = Out { f, pos: 0, total, ctl };

    // Header placeholder, then both slot regions raw, at the standard offsets with the
    // same slot size.
    out.zeros(ALIGN)?;
    fail(FailPoint::AfterPlaceholder)?;
    out.write(&slot_a)?;
    out.write(&slot_b)?;
    fail(FailPoint::AfterSlots)?;

    // Every chunk in TOC order, raw and CRC-checked; the guide's chunks replaced in place.
    let mut entries = Vec::with_capacity(otoc.chunks.len() + 2);
    let mut buf = vec![0u8; 1 << 20];
    let guide_entry = |id: &str, mime: &str, offset: u64, bytes: &[u8]| ChunkEntry {
        id: id.into(),
        mime: mime.into(),
        offset,
        length: bytes.len() as u64,
        crc32: crc32fast::hash(bytes),
        extra: Default::default(),
    };
    for (i, c) in otoc.chunks.iter().enumerate() {
        out.align()?;
        let offset = out.pos;
        if c.id == GUIDE_CHUNK {
            out.write(&chunks.guide_json)?;
            entries.push(guide_entry(GUIDE_CHUNK, GUIDE_MIME, offset, &chunks.guide_json));
        } else if c.id == CURVE_CHUNK {
            out.write(&chunks.curve)?;
            entries.push(guide_entry(CURVE_CHUNK, CURVE_MIME, offset, &chunks.curve));
        } else {
            out.copy_checked(src, &c.id, c.offset, c.length, c.crc32, &mut buf)?;
            entries.push(ChunkEntry {
                id: c.id.clone(),
                mime: c.mime.clone(),
                offset,
                length: c.length,
                crc32: c.crc32,
                extra: c.extra.clone(),
            });
        }
        if i == 0 {
            fail(FailPoint::MidChunks)?;
        }
    }
    // A new guide goes after the last chunk.
    if !has_guide {
        out.align()?;
        let offset = out.pos;
        out.write(&chunks.guide_json)?;
        entries.push(guide_entry(GUIDE_CHUNK, GUIDE_MIME, offset, &chunks.guide_json));
    }
    if !has_curve {
        out.align()?;
        let offset = out.pos;
        out.write(&chunks.curve)?;
        entries.push(guide_entry(CURVE_CHUNK, CURVE_MIME, offset, &chunks.curve));
    }
    fail(FailPoint::BeforeToc)?;

    // The TOC (unknown top-level keys kept), then the final header.
    let toc = Toc { chunks: entries, extra: otoc.extra.clone() };
    let toc_bytes = serde_json::to_vec(&toc).map_err(|e| FormatError::invalid(e.to_string()))?;
    out.align()?;
    let mut h = Header::new_v1(oh.slot_size);
    // 1.1 writers write minor 1; a file from a newer minor keeps its number (it still
    // holds that version's data, copied raw).
    h.minor = oh.minor.max(1);
    h.toc_offset = out.pos;
    h.toc_length = toc_bytes.len() as u64;
    h.toc_crc32 = crc32fast::hash(&toc_bytes);
    out.write(&toc_bytes)?;
    h.file_length = out.pos;
    fail(FailPoint::BeforeHeader)?;
    let f = out.f;
    f.seek(SeekFrom::Start(0))?;
    f.write_all(&h.encode())?;
    f.sync_all()?;
    p.header = h.clone();
    p.toc = toc;
    p.file_length = h.file_length;
    Ok(())
}

impl PreparedGuide {
    /// Section 7.1 steps 3-5. The caller must make sure nothing else writes the song
    /// meanwhile (in the app: the song is released by the player and the file lock is
    /// held; no settings save can run).
    ///
    /// 3. Copies both slot regions raw from the original again, so a settings save made
    ///    while step 2 was writing is not lost, and flushes the temporary file.
    /// 4. Checks the temporary file against the original **as it is now** (header and TOC
    ///    read again): same slot size, every chunk except the guide's with the same id,
    ///    type, length and CRC-32 in the same order, the new guide chunks' CRCs right, the
    ///    slot regions byte-identical. Any difference: the temporary file is deleted, the
    ///    original stays, and the error is [`FormatError::ChangedOnDisk`] (build again).
    /// 5. Renames the temporary file over the original (on Unix the folder is flushed
    ///    after).
    pub fn finish(mut self, ctl: &mut RewriteControl<'_>) -> Result<WrittenSong> {
        let fail_at = ctl.fail_at;
        let result = self.finish_inner(ctl);
        match &result {
            Ok(_) => self.keep = true,
            Err(e) if fail_at.is_some() && e.to_string().contains("simulated interruption") => self.keep = true,
            Err(_) => {} // Drop deletes the temporary file
        }
        result
    }

    fn finish_inner(&mut self, ctl: &mut RewriteControl<'_>) -> Result<WrittenSong> {
        let fail = |at: FailPoint| -> Result<()> { if ctl.fail_at == Some(at) { Err(simulated(at)) } else { Ok(()) } };
        if ctl.cancel.is_some_and(|c| c.load(Ordering::Relaxed)) {
            return Err(cancelled_error());
        }
        fail(FailPoint::BeforeSecondSlotCopy)?;
        let mut orig = File::open(&self.original)?;
        // A settings save can land between the slot copy and the final look at the
        // original (the app debounces slot saves); then the slots are copied again. Three
        // tries; a file that keeps changing, or changes otherwise, stops the swap.
        let mut attempt = 0;
        loop {
            attempt += 1;
            let (oh, otoc) = read_header_and_toc(&mut orig)?;
            if oh.slot_size != self.header.slot_size || !same_kept_chunks(&otoc, &self.toc) {
                return Err(FormatError::ChangedOnDisk);
            }
            // Step 3: the slots again, raw, from the original as it is now.
            let (slot_a, slot_b) = read_slot_regions(&mut orig, &oh)?;
            {
                let mut t = OpenOptions::new().read(true).write(true).open(&self.tmp)?;
                t.seek(SeekFrom::Start(self.header.slot_a_offset))?;
                t.write_all(&slot_a)?;
                t.seek(SeekFrom::Start(self.header.slot_b_offset))?;
                t.write_all(&slot_b)?;
                t.sync_all()?;
            }
            if ctl.fail_at == Some(FailPoint::TruncatedTemp) {
                let t = OpenOptions::new().write(true).open(&self.tmp)?;
                t.set_len(self.file_length / 2)?;
                t.sync_all()?;
                return Err(simulated(FailPoint::TruncatedTemp));
            }
            // Step 4: open the temporary file as a song file and compare.
            {
                let mut t = File::open(&self.tmp)?;
                let (th, ttoc) = read_header_and_toc(&mut t)?;
                if th != self.header || ttoc != self.toc {
                    return Err(FormatError::damaged("the temporary file does not read back as written"));
                }
                for (id, want) in [(GUIDE_CHUNK, self.guide_crc32), (CURVE_CHUNK, self.curve_crc32)] {
                    let c = ttoc.get(id).ok_or_else(|| FormatError::damaged(format!("temporary file lacks '{id}'")))?;
                    let b = read_exact_at(&mut t, c.offset, c.length)?;
                    if crc32fast::hash(&b) != want || c.crc32 != want {
                        return Err(FormatError::damaged(format!("chunk '{id}' did not read back from the temporary file")));
                    }
                }
                let (ta, tb) = read_slot_regions(&mut t, &th)?;
                if ta != slot_a || tb != slot_b {
                    return Err(FormatError::damaged("settings slots did not read back from the temporary file"));
                }
                if t.metadata()?.len() != self.file_length {
                    return Err(FormatError::damaged("the temporary file has the wrong length"));
                }
            }
            // The original must still be the file whose slots were just copied.
            let (oh2, _) = read_header_and_toc(&mut orig)?;
            let (a2, b2) = read_slot_regions(&mut orig, &oh2)?;
            if oh2 == oh && a2 == slot_a && b2 == slot_b {
                break;
            }
            if attempt >= 3 {
                return Err(FormatError::ChangedOnDisk);
            }
        }
        drop(orig);
        fail(FailPoint::BeforeRename)?;
        // Step 5.
        std::fs::rename(&self.tmp, &self.original)?;
        crate::writer::sync_parent_dir_pub(&self.original);
        self.keep = true;
        Ok(WrittenSong { path: self.original.clone(), header: self.header.clone(), toc: self.toc.clone() })
    }

    /// Leaves the temporary file on disk (tests that inspect an interrupted rewrite).
    pub fn keep_temp(mut self) -> PathBuf {
        self.keep = true;
        self.tmp.clone()
    }
}

/// Section 7.1 in one go: [`prepare_guide_rewrite`] then [`PreparedGuide::finish`]. For a
/// song nobody else has open (the command line, tests). The app splits the two.
pub fn add_guide(path: impl AsRef<Path>, chunks: &GuideChunks, ctl: &mut RewriteControl<'_>) -> Result<WrittenSong> {
    let p = prepare_guide_rewrite(path, chunks, ctl)?;
    p.finish(ctl)
}

/// SHA-256 (lowercase hex) of `len` bytes at `off` of a file (a chunk's bytes as stored).
pub fn sha256_of_range(path: impl AsRef<Path>, off: u64, len: u64) -> Result<String> {
    use sha2::Digest;
    let mut r = ChunkReader::new(File::open(path)?, off, len);
    let mut h = sha2::Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = r.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(hex(&h.finalize()))
}

/// Lowercase hex of bytes.
pub fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

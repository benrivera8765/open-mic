//! A `Read + Seek` view of one chunk of an open song file.
//!
//! Each reader owns its own file handle and reads with positional I/O (`pread` on Unix,
//! `ReadFile` with an explicit offset on Windows), so several readers (one per stem, on
//! different threads) never disturb each other, and nothing is read into memory ahead
//! of the decoder. The handle is a duplicate of the one the [`crate::SongFile`] was
//! opened with, so the reader always sees the same file the table of contents came
//! from, even if the path has since been replaced by an atomic rewrite.
//!
//! With the `symphonia` feature (default) it implements `symphonia_core::io::MediaSource`,
//! so a stem chunk can be wrapped in a `MediaSourceStream` and fed to the FLAC demuxer.

use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};

/// Reads bytes `[start, start + len)` of a file as if they were a file of their own.
#[derive(Debug)]
pub struct ChunkReader {
    file: File,
    start: u64,
    len: u64,
    pos: u64,
}

impl ChunkReader {
    /// A view of `len` bytes at absolute `start` of `file`. The handle is used only with
    /// positional reads; its own cursor is never relied on.
    pub fn new(file: File, start: u64, len: u64) -> ChunkReader {
        ChunkReader { file, start, len, pos: 0 }
    }

    /// Another reader over the same chunk of the same open file (a duplicated handle),
    /// positioned at the chunk start. Independent of `self`: positional reads only.
    pub fn reopen(&self) -> io::Result<ChunkReader> {
        Ok(ChunkReader { file: self.file.try_clone()?, start: self.start, len: self.len, pos: 0 })
    }

    /// Chunk length in bytes.
    pub fn len(&self) -> u64 {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Absolute offset of the chunk in the file.
    pub fn file_offset(&self) -> u64 {
        self.start
    }

    /// Reads exactly `buf.len()` bytes at chunk-relative `offset` without moving the
    /// reader's position.
    pub fn read_exact_at(&self, mut buf: &mut [u8], mut offset: u64) -> io::Result<()> {
        if offset.checked_add(buf.len() as u64).is_none_or(|end| end > self.len) {
            return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "read past the end of the chunk"));
        }
        while !buf.is_empty() {
            let n = read_at(&self.file, buf, self.start + offset)?;
            if n == 0 {
                return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "file ended inside the chunk"));
            }
            buf = &mut buf[n..];
            offset += n as u64;
        }
        Ok(())
    }
}

impl Read for ChunkReader {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.pos >= self.len || buf.is_empty() {
            return Ok(0);
        }
        let want = (buf.len() as u64).min(self.len - self.pos) as usize;
        let n = read_at(&self.file, &mut buf[..want], self.start + self.pos)?;
        self.pos += n as u64;
        Ok(n)
    }
}

impl Seek for ChunkReader {
    fn seek(&mut self, to: SeekFrom) -> io::Result<u64> {
        let new = match to {
            SeekFrom::Start(p) => Some(p),
            SeekFrom::End(d) => self.len.checked_add_signed(d),
            SeekFrom::Current(d) => self.pos.checked_add_signed(d),
        };
        match new {
            Some(p) => {
                // Like a file, seeking past the end is allowed; reads there return 0.
                self.pos = p;
                Ok(p)
            }
            None => Err(io::Error::new(io::ErrorKind::InvalidInput, "seek to a negative position")),
        }
    }

    fn stream_position(&mut self) -> io::Result<u64> {
        Ok(self.pos)
    }
}

#[cfg(unix)]
fn read_at(f: &File, buf: &mut [u8], off: u64) -> io::Result<usize> {
    use std::os::unix::fs::FileExt;
    loop {
        match f.read_at(buf, off) {
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            r => return r,
        }
    }
}

#[cfg(windows)]
fn read_at(f: &File, buf: &mut [u8], off: u64) -> io::Result<usize> {
    use std::os::windows::fs::FileExt;
    // seek_read passes the offset in an OVERLAPPED structure: positional, safe to call
    // from several handles to the same file at once. (It moves the shared file pointer
    // as a side effect, which nothing here relies on.)
    match f.seek_read(buf, off) {
        // Reading at or past EOF reports ERROR_HANDLE_EOF on some Windows versions.
        Err(e) if e.raw_os_error() == Some(38) => Ok(0),
        r => r,
    }
}

#[cfg(not(any(unix, windows)))]
fn read_at(mut f: &File, buf: &mut [u8], off: u64) -> io::Result<usize> {
    f.seek(SeekFrom::Start(off))?;
    f.read(buf)
}

#[cfg(feature = "symphonia")]
impl symphonia_core::io::MediaSource for ChunkReader {
    fn is_seekable(&self) -> bool {
        true
    }

    fn byte_len(&self) -> Option<u64> {
        Some(self.len)
    }
}

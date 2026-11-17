//! Errors, split into kinds a user interface can explain in plain language.

use std::fmt;
use std::io;

/// The broad kind of a [`FormatError`], for mapping to a message or an icon in the UI.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ErrorKind {
    /// The file is not a `.jksong` file at all (wrong magic bytes).
    NotASongFile,
    /// The file was made by a newer, incompatible version (higher major version).
    NewerVersion,
    /// The file is shorter than it says it is: an incomplete copy.
    Truncated,
    /// A checksum or a structural check failed: the file is damaged.
    Damaged,
    /// A chunk the manifest needs is not in the file.
    MissingChunk,
    /// A settings payload does not fit in a settings slot. The caller should rewrite the
    /// whole file instead (see [`crate::writer::rewrite_song`]), which enlarges the slots.
    SlotPayloadTooLarge,
    /// The file changed on disk since it was opened (another program rewrote it).
    ChangedOnDisk,
    /// The caller asked the writer to store something the format cannot hold.
    InvalidInput,
    /// An operating-system I/O error (permission denied, disk full, stick removed...).
    Io,
}

/// Every error this crate returns.
#[derive(Debug)]
#[non_exhaustive]
pub enum FormatError {
    /// Wrong magic bytes. Either not a song file, or a song file damaged by a text-mode
    /// transfer (the magic is designed to detect line-ending conversion).
    NotASongFile,
    /// The header's major version is higher than this reader supports.
    NewerVersion { major: u16, minor: u16 },
    /// The file is shorter than its header says (or too short to hold a header).
    /// `expected` is `None` when the file is too short to even read `file_length`.
    Truncated { expected: Option<u64>, actual: u64 },
    /// A checksum or structural check failed. `detail` says which, for the log.
    Damaged { detail: String },
    /// A chunk that the manifest refers to (or the manifest itself) is missing.
    MissingChunk { id: String },
    /// The settings payload is `needed` bytes but a slot holds only `capacity` payload bytes.
    SlotPayloadTooLarge { needed: usize, capacity: usize },
    /// The file on disk no longer matches what was opened.
    ChangedOnDisk,
    /// Writer misuse: duplicate chunk ids, manifest referring to a chunk not given, etc.
    InvalidInput(String),
    /// Underlying I/O error.
    Io(io::Error),
}

pub type Result<T> = std::result::Result<T, FormatError>;

impl FormatError {
    pub(crate) fn damaged(detail: impl Into<String>) -> Self {
        FormatError::Damaged { detail: detail.into() }
    }

    pub(crate) fn invalid(detail: impl Into<String>) -> Self {
        FormatError::InvalidInput(detail.into())
    }

    /// The broad kind of this error.
    pub fn kind(&self) -> ErrorKind {
        match self {
            FormatError::NotASongFile => ErrorKind::NotASongFile,
            FormatError::NewerVersion { .. } => ErrorKind::NewerVersion,
            FormatError::Truncated { .. } => ErrorKind::Truncated,
            FormatError::Damaged { .. } => ErrorKind::Damaged,
            FormatError::MissingChunk { .. } => ErrorKind::MissingChunk,
            FormatError::SlotPayloadTooLarge { .. } => ErrorKind::SlotPayloadTooLarge,
            FormatError::ChangedOnDisk => ErrorKind::ChangedOnDisk,
            FormatError::InvalidInput(_) => ErrorKind::InvalidInput,
            FormatError::Io(_) => ErrorKind::Io,
        }
    }

    /// A plain-language sentence for the user. Technical detail goes to the log via
    /// `Display` / `Debug`.
    pub fn user_message(&self) -> String {
        match self {
            FormatError::NotASongFile => {
                "This is not a Jukeoke song file (or it was damaged when it was copied as text).".into()
            }
            FormatError::NewerVersion { .. } => {
                "This song was made by a newer version of Jukeoke; please update.".into()
            }
            FormatError::Truncated { .. } => {
                "This song file is incomplete, probably an unfinished copy. Copy it again from the original.".into()
            }
            FormatError::Damaged { .. } | FormatError::MissingChunk { .. } => {
                "This song file is damaged. Copy it again from the original, or make it again in the Studio.".into()
            }
            FormatError::SlotPayloadTooLarge { .. } => {
                "The song's saved settings are too large for a quick save; the file needs a full rewrite.".into()
            }
            FormatError::ChangedOnDisk => {
                "This song file was changed by another program. Open it again.".into()
            }
            FormatError::InvalidInput(_) => "The song file could not be written (internal error).".into(),
            FormatError::Io(e) => match e.kind() {
                io::ErrorKind::NotFound => "The song file could not be found.".into(),
                io::ErrorKind::PermissionDenied => {
                    "Jukeoke is not allowed to read or write this song file (permission denied, or the drive is read-only).".into()
                }
                _ => format!("The song file could not be read or written: {e}"),
            },
        }
    }
}

impl fmt::Display for FormatError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FormatError::NotASongFile => write!(f, "not a .jksong file (bad magic)"),
            FormatError::NewerVersion { major, minor } => {
                write!(f, "song file version {major}.{minor} is newer than this reader (major {})", crate::FORMAT_MAJOR)
            }
            FormatError::Truncated { expected: Some(e), actual } => {
                write!(f, "song file truncated: header says {e} bytes, file has {actual}")
            }
            FormatError::Truncated { expected: None, actual } => {
                write!(f, "song file truncated: only {actual} bytes, too short for a header")
            }
            FormatError::Damaged { detail } => write!(f, "song file damaged: {detail}"),
            FormatError::MissingChunk { id } => write!(f, "song file is missing required chunk '{id}'"),
            FormatError::SlotPayloadTooLarge { needed, capacity } => {
                write!(f, "settings payload of {needed} bytes does not fit in a slot ({capacity} bytes available)")
            }
            FormatError::ChangedOnDisk => write!(f, "song file changed on disk since it was opened"),
            FormatError::InvalidInput(s) => write!(f, "invalid input: {s}"),
            FormatError::Io(e) => write!(f, "I/O error: {e}"),
        }
    }
}

impl std::error::Error for FormatError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            FormatError::Io(e) => Some(e),
            _ => None,
        }
    }
}

impl From<io::Error> for FormatError {
    fn from(e: io::Error) -> Self {
        FormatError::Io(e)
    }
}

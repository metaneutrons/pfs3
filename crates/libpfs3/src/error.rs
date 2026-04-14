//! Error types for libpfs3.

/// Result type alias using the PFS3 [`Error`].
pub type Result<T> = std::result::Result<T, Error>;

/// PFS3 library error type.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("{0}: data too short")]
    TooShort(&'static str),

    #[error("{0}: bad magic 0x{1:08x}")]
    BadMagic(&'static str, u32),

    #[error("{0}: expected block id 0x{1:04x}, got 0x{2:04x}")]
    BadBlockId(&'static str, u16, u16),

    #[error("block {0} out of range")]
    BlockOutOfRange(u64),

    #[error("anode {0} not found")]
    AnodeNotFound(u32),

    #[error("not found: {0}")]
    NotFound(String),

    #[error("already exists: {0}")]
    AlreadyExists(String),

    #[error("disk full: {0}")]
    DiskFull(String),

    #[error("corrupt filesystem: {0}")]
    Corrupt(String),

    #[error("not a directory")]
    NotADirectory,

    #[error("directory not empty")]
    NotEmpty,

    #[error("invalid partition: {0}")]
    InvalidPartition(String),

    #[error("device is read-only")]
    ReadOnly,
}

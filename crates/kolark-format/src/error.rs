//! Format-related errors

use thiserror::Error;

#[derive(Error, Debug)]
pub enum FormatError {
    #[error("invalid magic: expected KOLA, got {0:?}")]
    InvalidMagic([u8; 4]),

    #[error("unsupported version: {0}, expected 1")]
    UnsupportedVersion(u32),

    #[error("unsupported flags: {0}, expected 0")]
    UnsupportedFlags(u32),

    #[error("invalid superblock: {0}")]
    InvalidSuperblock(String),

    #[error("invalid footer: {0}")]
    InvalidFooter(String),

    #[error("invalid offset: {0}")]
    InvalidOffset(String),

    #[error("integer overflow: {0}")]
    IntegerOverflow(String),

    #[error("invalid varint: {0}")]
    InvalidVarint(String),

    #[error("invalid encoding: {0}")]
    InvalidEncoding(String),

    #[error("invalid codec: {0}")]
    InvalidCodec(String),

    #[error("hash mismatch: expected {expected:?}, got {actual:?}")]
    HashMismatch { expected: Vec<u8>, actual: Vec<u8> },

    #[error("invalid chunk index: {0}")]
    InvalidChunkIndex(String),

    #[error("invalid metadata: {0}")]
    InvalidMetadata(String),

    #[error("invalid path: {0}")]
    InvalidPath(String),

    #[error("path collision: {0}")]
    PathCollision(String),

    #[error("resource limit exceeded: {0}")]
    ResourceLimitExceeded(String),

    #[error("destination exists: {0}")]
    DestinationExists(String),

    #[error("unsupported feature: {0}")]
    UnsupportedFeature(String),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("decompression error: {0}")]
    DecompressionError(String),
}

pub type FormatResult<T> = Result<T, FormatError>;

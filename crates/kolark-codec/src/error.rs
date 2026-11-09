//! KolArk codec errors

use thiserror::Error;

#[derive(Error, Debug, Clone, PartialEq, Eq)]
pub enum KolArkCodecError {
    #[error("invalid configuration: {0}")]
    InvalidConfig(String),

    #[error("varint error: {0}")]
    Varint(#[from] crate::varint::VarintError),

    #[error("LZ error: {0}")]
    Lz(#[from] crate::lz::LzError),

    #[error("CDC error: {0}")]
    Cdc(String),

    #[error("BLAKE3 error: {0}")]
    Blake3(String),
}

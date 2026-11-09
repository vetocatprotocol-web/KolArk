//! KolArk core codecs: BLAKE3, Gear CDC, KolArk-LZ, varint

pub mod blake3;
pub mod cdc;
pub mod error;
pub mod gear;
pub mod lz;
pub mod varint;

pub use blake3::{blake3_hash, blake3_hash_streaming, Blake3Hasher};
pub use cdc::{cdc_split, CdcConfig};
pub use error::KolArkCodecError;
pub use gear::{gear_table, generate_gear_table, GEAR_TABLE_256};
pub use lz::{lz_compress, lz_decompress, LzError};
pub use varint::{read_svarint, read_varint, write_svarint, write_varint, VarintError};

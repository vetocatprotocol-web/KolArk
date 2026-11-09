//! KolArk binary format types and serialization

pub mod chunk_index;
pub mod chunk_record;
pub mod column;
pub mod error;
pub mod footer;
pub mod header;
pub mod superblock;

pub use chunk_index::{ChunkIndex, ChunkIndexEntry, CHUNK_INDEX_ENTRY_SIZE};
pub use chunk_record::{ChunkRecord, CHUNK_FLAG_LZ, CHUNK_FLAG_RAW, CHUNK_RECORD_HEADER_SIZE};
pub use column::{Column, ColumnCodec, ColumnEncoding, COLUMN_FIXED_HEADER_SIZE};
pub use error::{FormatError, FormatResult};
pub use footer::{Footer, FOOTER_SIZE};
pub use header::{Header, HEADER_SIZE};
pub use superblock::{Superblock, SUPERBLOCK_SIZE};

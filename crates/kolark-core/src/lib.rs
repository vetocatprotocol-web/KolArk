//! KolArk core: reader, writer, metadata, index

pub mod index;
pub mod metadata;
pub mod reader;
pub mod writer;

pub use index::{build_chunk_index, validate_chunk_index};
pub use metadata::{decode_columns, encode_columns, ColumnData};
pub use reader::{
    open_archive, ArchiveEntry, ArchiveInfo, ArchiveReader, ReaderOptions, VerifyLevel,
    VerifyReport,
};
pub use writer::{create_archive, ArchiveSummary, CreateOptions, InputEntry};

//! Archive reader

use byteorder::{LittleEndian, ReadBytesExt};
use kolark_codec::blake3::Blake3Hasher;
use kolark_codec::{blake3_hash, lz_decompress};
use kolark_format::{ChunkIndex, Column, Footer, FormatError, FormatResult, Header, Superblock};
use std::io::{Read, Seek};

fn read_varint<R: Read>(reader: &mut R) -> FormatResult<u64> {
    kolark_codec::varint::read_varint(reader)
        .map_err(|error| FormatError::InvalidVarint(error.to_string()))
}

fn read_svarint<R: Read>(reader: &mut R) -> FormatResult<i64> {
    kolark_codec::varint::read_svarint(reader)
        .map_err(|error| FormatError::InvalidVarint(error.to_string()))
}

#[derive(Debug, Clone, Default)]
pub struct ReaderOptions {
    pub max_files: Option<u32>,
    pub max_chunks: Option<u32>,
    pub max_memory: Option<u64>,
    pub max_output_bytes: Option<u64>,
}

#[derive(Debug, Clone)]
pub struct ArchiveEntry {
    pub path: std::path::PathBuf,
    pub size: u64,
    pub mtime: i64,
    pub mode: u16,
    pub content_hash: [u8; 32],
    pub chunk_refs: Vec<u32>,
}

#[derive(Debug, Clone)]
pub struct ArchiveInfo {
    pub format_version: u32,
    pub file_count: u32,
    pub chunk_count: u32,
    pub archive_size: u64,
    pub chunk_size_min: u32,
    pub chunk_size_avg: u32,
    pub chunk_size_max: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VerifyLevel {
    Quick,
    Full,
}

#[derive(Debug, Clone)]
pub struct VerifyReport {
    pub valid: bool,
    pub issues: Vec<String>,
}

pub struct ArchiveReader<R: Read + Seek> {
    reader: R,
    header: Header,
    superblock: Superblock,
    footer: Footer,
    chunk_index: ChunkIndex,
    entries: Vec<ArchiveEntry>,
    file_size: u64,
    max_memory: u64,
}

impl<R: Read + Seek> ArchiveReader<R> {
    pub fn new(reader: R) -> FormatResult<Self> {
        Self::new_with_options(reader, &ReaderOptions::default())
    }

    pub fn new_with_options(mut reader: R, options: &ReaderOptions) -> FormatResult<Self> {
        let file_size = reader.seek(std::io::SeekFrom::End(0))?;
        reader.seek(std::io::SeekFrom::Start(0))?;

        let header = Header::read(&mut reader)?;
        header.validate()?;

        let superblock = Superblock::read(&mut reader)?;
        superblock.validate()?;

        let footer = Footer::read(&mut reader, file_size)?;
        footer.validate_file_size(file_size)?;

        // Verify hash_all
        reader.seek(std::io::SeekFrom::Start(0))?;
        let mut buffer = vec![0u8; 8192];
        let hash_all_start = 0;
        let hash_all_end = file_size - kolark_format::footer::FOOTER_SIZE;

        reader.seek(std::io::SeekFrom::Start(hash_all_start))?;
        let mut hasher = Blake3Hasher::new();
        let mut remaining = hash_all_end;
        while remaining > 0 {
            let to_read = (buffer.len() as u64).min(remaining);
            reader.read_exact(&mut buffer[..to_read as usize])?;
            hasher.update(&buffer[..to_read as usize]);
            remaining -= to_read;
        }
        let computed_hash = hasher.finalize();

        if computed_hash != footer.hash_all {
            return Err(FormatError::HashMismatch {
                expected: footer.hash_all.to_vec(),
                actual: computed_hash.to_vec(),
            });
        }

        let footer_start = file_size
            .checked_sub(kolark_format::FOOTER_SIZE)
            .ok_or_else(|| FormatError::InvalidFooter("file is shorter than footer".to_string()))?;
        let index_bytes = footer_start
            .checked_sub(footer.offset_chunk_index)
            .ok_or_else(|| {
                FormatError::InvalidOffset("chunk index starts past footer".to_string())
            })?;
        if index_bytes < 4 || (index_bytes - 4) % kolark_format::CHUNK_INDEX_ENTRY_SIZE as u64 != 0
        {
            return Err(FormatError::InvalidChunkIndex(
                "chunk index region has an invalid length".to_string(),
            ));
        }

        reader.seek(std::io::SeekFrom::Start(footer.offset_chunk_index))?;
        let maximum_entries =
            u32::try_from((index_bytes - 4) / kolark_format::CHUNK_INDEX_ENTRY_SIZE as u64)
                .unwrap_or(u32::MAX);
        if options.max_memory.is_some_and(|limit| index_bytes > limit) {
            return Err(FormatError::ResourceLimitExceeded(
                "chunk index exceeds configured memory limit".to_string(),
            ));
        }
        let max_chunks = options.max_chunks.unwrap_or(u32::MAX).min(maximum_entries);
        let chunk_index = ChunkIndex::read_bounded(&mut reader, max_chunks)?;
        if chunk_index.total_size() != index_bytes {
            return Err(FormatError::InvalidChunkIndex(
                "chunk index count does not match its encoded size".to_string(),
            ));
        }
        chunk_index.validate(footer.offset_table_meta)?;
        validate_chunk_store(
            &mut reader,
            &chunk_index,
            footer.offset_table_meta,
            &superblock,
        )?;

        // Read table metadata (6 columns)
        reader.seek(std::io::SeekFrom::Start(footer.offset_table_meta))?;
        let mut columns: Vec<Column> = Vec::with_capacity(6);
        let memory_limit = options.max_memory.unwrap_or(u64::MAX);
        let mut stored_column_bytes = 0u64;
        for expected_id in 0..6 {
            let remaining_memory = memory_limit.saturating_sub(stored_column_bytes);
            let column =
                Column::read_bounded(&mut reader, footer.offset_chunk_index, remaining_memory)?;
            if column.col_id != expected_id {
                return Err(FormatError::InvalidMetadata(format!(
                    "expected col_id {}, got {}",
                    expected_id, column.col_id
                )));
            }
            let expected_count = columns
                .first()
                .map(|first| first.element_count)
                .unwrap_or(column.element_count);
            column.validate(expected_count)?;
            if expected_id == 0
                && options
                    .max_files
                    .is_some_and(|limit| column.element_count > limit)
            {
                return Err(FormatError::ResourceLimitExceeded(
                    "file count exceeds configured limit".to_string(),
                ));
            }
            stored_column_bytes = stored_column_bytes
                .checked_add(column.dict_size as u64)
                .and_then(|bytes| bytes.checked_add(column.compressed_size as u64))
                .ok_or_else(|| FormatError::IntegerOverflow("metadata memory size".to_string()))?;
            if stored_column_bytes > memory_limit {
                return Err(FormatError::ResourceLimitExceeded(
                    "metadata columns exceed configured memory limit".to_string(),
                ));
            }
            columns.push(column);
        }

        // Verify we're at chunk index offset
        let pos = reader.stream_position()?;
        if pos != footer.offset_chunk_index {
            return Err(FormatError::InvalidMetadata(format!(
                "table metadata ends at {} but chunk index starts at {}",
                pos, footer.offset_chunk_index
            )));
        }

        // Decode columns to build entries
        let entries = decode_columns(&columns, &chunk_index, memory_limit)?;
        let total_output = entries
            .iter()
            .try_fold(0u64, |total, entry| total.checked_add(entry.size))
            .ok_or_else(|| FormatError::IntegerOverflow("total file output size".to_string()))?;
        if options
            .max_output_bytes
            .is_some_and(|limit| total_output > limit)
        {
            return Err(FormatError::ResourceLimitExceeded(
                "archive output exceeds configured byte limit".to_string(),
            ));
        }

        Ok(Self {
            reader,
            header,
            superblock,
            footer,
            chunk_index,
            entries,
            file_size,
            max_memory: options.max_memory.unwrap_or(u64::MAX),
        })
    }

    pub fn entries(&self) -> &[ArchiveEntry] {
        &self.entries
    }

    pub fn info(&self) -> ArchiveInfo {
        ArchiveInfo {
            format_version: self.header.version,
            file_count: self.entries.len() as u32,
            chunk_count: self.chunk_index.count,
            archive_size: self.file_size,
            chunk_size_min: self.superblock.chunk_size_min,
            chunk_size_avg: self.superblock.chunk_size_avg,
            chunk_size_max: self.superblock.chunk_size_max,
        }
    }

    pub fn read_chunk(&mut self, chunk_index: u32) -> FormatResult<Vec<u8>> {
        let entry = self
            .chunk_index
            .entries
            .get(chunk_index as usize)
            .ok_or_else(|| {
                FormatError::InvalidChunkIndex(format!(
                    "chunk reference {} is outside the index",
                    chunk_index
                ))
            })?;
        let offset = entry.offset;
        let expected_hash = entry.hash;
        let next_offset = self
            .chunk_index
            .entries
            .iter()
            .map(|candidate| candidate.offset)
            .filter(|candidate| *candidate > offset)
            .min()
            .unwrap_or(self.footer.offset_table_meta);

        self.reader.seek(std::io::SeekFrom::Start(offset))?;
        let mut hash = [0u8; 32];
        self.reader.read_exact(&mut hash)?;
        let size_compressed = self.reader.read_u32::<LittleEndian>()?;
        let size_original = self.reader.read_u32::<LittleEndian>()?;
        let flags = self.reader.read_u8()?;
        let record_end = offset
            .checked_add(kolark_format::CHUNK_RECORD_HEADER_SIZE as u64)
            .and_then(|end| end.checked_add(size_compressed as u64))
            .ok_or_else(|| FormatError::IntegerOverflow("chunk record end".to_string()))?;

        if record_end != next_offset {
            return Err(FormatError::InvalidChunkIndex(format!(
                "chunk record at {} ends at {}, expected {}",
                offset, record_end, next_offset
            )));
        }
        if hash != expected_hash {
            return Err(FormatError::InvalidChunkIndex(format!(
                "chunk index hash does not match record at {}",
                offset
            )));
        }
        if size_original == 0 || size_original > self.superblock.chunk_size_max {
            return Err(FormatError::InvalidEncoding(format!(
                "invalid original chunk size {}",
                size_original
            )));
        }
        if size_original as u64 > self.max_memory || size_compressed as u64 > self.max_memory {
            return Err(FormatError::ResourceLimitExceeded(
                "chunk exceeds configured memory limit".to_string(),
            ));
        }
        if size_compressed as u64 > next_offset.saturating_sub(offset + 41) {
            return Err(FormatError::InvalidEncoding(
                "chunk payload exceeds its record boundary".to_string(),
            ));
        }

        let mut compressed = vec![0u8; size_compressed as usize];
        self.reader.read_exact(&mut compressed)?;
        let raw = match flags {
            kolark_format::CHUNK_FLAG_RAW if size_compressed == size_original => compressed,
            kolark_format::CHUNK_FLAG_RAW => {
                return Err(FormatError::InvalidEncoding(
                    "raw chunk has inconsistent sizes".to_string(),
                ));
            }
            kolark_format::CHUNK_FLAG_LZ if size_compressed < size_original => {
                lz_decompress(&compressed, size_original as usize)
                    .map_err(|error| FormatError::DecompressionError(error.to_string()))?
            }
            kolark_format::CHUNK_FLAG_LZ => {
                return Err(FormatError::InvalidEncoding(
                    "LZ chunk must be smaller than its original data".to_string(),
                ));
            }
            _ => {
                return Err(FormatError::InvalidEncoding(format!(
                    "unsupported chunk flags 0x{flags:02x}"
                )));
            }
        };
        let actual_hash = blake3_hash(&raw);
        if actual_hash != hash {
            return Err(FormatError::HashMismatch {
                expected: hash.to_vec(),
                actual: actual_hash.to_vec(),
            });
        }
        Ok(raw)
    }

    pub fn verify(&mut self, level: VerifyLevel) -> FormatResult<VerifyReport> {
        if level == VerifyLevel::Quick {
            return Ok(VerifyReport {
                valid: true,
                issues: Vec::new(),
            });
        }

        for index in 0..self.chunk_index.count {
            self.read_chunk(index)?;
        }
        for index in 0..self.entries.len() {
            let entry = self.entries[index].clone();
            let mut hasher = Blake3Hasher::new();
            let mut total_size = 0u64;
            for chunk_ref in &entry.chunk_refs {
                let chunk = self.read_chunk(*chunk_ref)?;
                total_size = total_size
                    .checked_add(chunk.len() as u64)
                    .ok_or_else(|| FormatError::IntegerOverflow("file size".to_string()))?;
                hasher.update(&chunk);
            }
            let actual_hash = hasher.finalize();
            if total_size != entry.size {
                return Err(FormatError::InvalidMetadata(format!(
                    "reconstructed size for {} is {}, expected {}",
                    entry.path.display(),
                    total_size,
                    entry.size
                )));
            }
            if actual_hash != entry.content_hash {
                return Err(FormatError::HashMismatch {
                    expected: entry.content_hash.to_vec(),
                    actual: actual_hash.to_vec(),
                });
            }
        }

        Ok(VerifyReport {
            valid: true,
            issues: Vec::new(),
        })
    }
}

pub fn open_archive<R: Read + Seek>(
    reader: R,
    options: &ReaderOptions,
) -> FormatResult<ArchiveReader<R>> {
    ArchiveReader::new_with_options(reader, options)
}

fn validate_chunk_store<R: Read + Seek>(
    reader: &mut R,
    index: &ChunkIndex,
    chunk_store_end: u64,
    superblock: &Superblock,
) -> FormatResult<()> {
    let mut physical_entries = index.entries.iter().collect::<Vec<_>>();
    physical_entries.sort_by_key(|entry| entry.offset);
    let mut expected_offset = 48u64;

    for entry in physical_entries {
        if entry.offset != expected_offset {
            return Err(FormatError::InvalidChunkIndex(format!(
                "chunk record begins at {}, expected {}",
                entry.offset, expected_offset
            )));
        }

        let header_end = entry
            .offset
            .checked_add(kolark_format::CHUNK_RECORD_HEADER_SIZE as u64)
            .ok_or_else(|| FormatError::IntegerOverflow("chunk header end".to_string()))?;
        if header_end > chunk_store_end {
            return Err(FormatError::InvalidChunkIndex(
                "truncated chunk record header".to_string(),
            ));
        }

        reader.seek(std::io::SeekFrom::Start(entry.offset))?;
        let mut hash = [0u8; 32];
        reader.read_exact(&mut hash)?;
        let size_compressed = reader.read_u32::<LittleEndian>()?;
        let size_original = reader.read_u32::<LittleEndian>()?;
        let flags = reader.read_u8()?;
        if hash != entry.hash {
            return Err(FormatError::InvalidChunkIndex(format!(
                "index hash differs from chunk record at {}",
                entry.offset
            )));
        }
        if size_original == 0 || size_original > superblock.chunk_size_max {
            return Err(FormatError::InvalidEncoding(format!(
                "invalid chunk original size {size_original}"
            )));
        }
        match flags {
            kolark_format::CHUNK_FLAG_RAW if size_compressed == size_original => {}
            kolark_format::CHUNK_FLAG_LZ if size_compressed < size_original => {}
            kolark_format::CHUNK_FLAG_RAW => {
                return Err(FormatError::InvalidEncoding(
                    "raw chunk has inconsistent sizes".to_string(),
                ));
            }
            kolark_format::CHUNK_FLAG_LZ => {
                return Err(FormatError::InvalidEncoding(
                    "LZ chunk must be smaller than its original data".to_string(),
                ));
            }
            _ => {
                return Err(FormatError::InvalidEncoding(format!(
                    "unsupported chunk flags 0x{flags:02x}"
                )));
            }
        }
        expected_offset = header_end
            .checked_add(size_compressed as u64)
            .ok_or_else(|| FormatError::IntegerOverflow("chunk record end".to_string()))?;
        if expected_offset > chunk_store_end {
            return Err(FormatError::InvalidChunkIndex(
                "chunk record exceeds the chunk store".to_string(),
            ));
        }
    }

    if expected_offset != chunk_store_end {
        return Err(FormatError::InvalidChunkIndex(format!(
            "chunk store ends at {}, expected {}",
            chunk_store_end, expected_offset
        )));
    }
    Ok(())
}

fn decode_column_payload(column: &Column, max_output: u64) -> FormatResult<Vec<u8>> {
    if column.data.len() != column.compressed_size as usize {
        return Err(FormatError::InvalidMetadata(format!(
            "column {} compressed_size does not match payload",
            column.col_id
        )));
    }

    match column.codec {
        kolark_format::ColumnCodec::Raw => {
            if column.compressed_size != column.original_size {
                return Err(FormatError::InvalidMetadata(format!(
                    "raw column {} has inconsistent sizes",
                    column.col_id
                )));
            }
            Ok(column.data.clone())
        }
        kolark_format::ColumnCodec::KolArkLZ => {
            if column.original_size as u64 > max_output {
                return Err(FormatError::ResourceLimitExceeded(format!(
                    "decompressed column {} exceeds memory limit",
                    column.col_id
                )));
            }
            if column.compressed_size >= column.original_size {
                return Err(FormatError::InvalidMetadata(format!(
                    "LZ column {} is not smaller than its original data",
                    column.col_id
                )));
            }
            lz_decompress(&column.data, column.original_size as usize).map_err(|error| {
                FormatError::DecompressionError(format!("column {}: {error}", column.col_id))
            })
        }
    }
}

fn validate_metadata_path(path: &str) -> FormatResult<()> {
    let components = path.split('/').collect::<Vec<_>>();
    let first = components.first().copied().unwrap_or_default();
    let scheme_like = first.split_once(':').is_some_and(|(scheme, _)| {
        !scheme.is_empty()
            && scheme.as_bytes()[0].is_ascii_alphabetic()
            && scheme
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'.' | b'-'))
    });
    if path.is_empty()
        || path.len() > 4096
        || path.starts_with('/')
        || path.contains('\0')
        || path.contains('\\')
        || components
            .iter()
            .any(|component| component.is_empty() || *component == "." || *component == "..")
        || scheme_like
    {
        return Err(FormatError::InvalidPath(format!(
            "path does not meet KolArk v1 requirements: {path}"
        )));
    }
    Ok(())
}

fn decode_columns(
    columns: &[kolark_format::Column],
    chunk_index: &ChunkIndex,
    max_memory: u64,
) -> FormatResult<Vec<ArchiveEntry>> {
    if columns.len() != 6 {
        return Err(FormatError::InvalidMetadata(
            "expected 6 columns".to_string(),
        ));
    }

    let element_count = columns[0].element_count;
    let stored_size = columns
        .iter()
        .try_fold(0u64, |total, column| {
            total
                .checked_add(column.dict_size as u64)
                .and_then(|value| value.checked_add(column.compressed_size as u64))
        })
        .ok_or_else(|| FormatError::IntegerOverflow("stored metadata size".to_string()))?;
    let decoded_size = columns
        .iter()
        .try_fold(0u64, |total, column| {
            total.checked_add(column.original_size as u64)
        })
        .ok_or_else(|| FormatError::IntegerOverflow("decoded metadata size".to_string()))?;
    if stored_size
        .checked_add(decoded_size)
        .is_none_or(|required| required > max_memory)
    {
        return Err(FormatError::ResourceLimitExceeded(
            "metadata exceeds configured memory limit".to_string(),
        ));
    }
    let expected_encodings = [
        kolark_format::ColumnEncoding::DictionaryVarint,
        kolark_format::ColumnEncoding::DeltaSvarint,
        kolark_format::ColumnEncoding::Varint,
        kolark_format::ColumnEncoding::Raw,
        kolark_format::ColumnEncoding::Varint,
        kolark_format::ColumnEncoding::Raw,
    ];
    let mut payloads = Vec::with_capacity(6);
    for (column_id, column) in columns.iter().enumerate() {
        if column.col_id as usize != column_id
            || column.element_count != element_count
            || column.encoding != expected_encodings[column_id]
            || column.reserved != 0
        {
            return Err(FormatError::InvalidMetadata(format!(
                "column {} has inconsistent ID, encoding, count, or reserved field",
                column_id
            )));
        }
        if column_id != 0 && (column.dict_size != 0 || !column.dictionary.is_empty()) {
            return Err(FormatError::InvalidMetadata(format!(
                "column {} must not have a dictionary",
                column_id
            )));
        }
        payloads.push(decode_column_payload(column, max_memory)?);
    }

    let path_column = &columns[0];
    if path_column.dictionary.len() != path_column.dict_size as usize
        || path_column.dictionary.len() < 4
    {
        return Err(FormatError::InvalidMetadata(
            "path dictionary has an invalid size".to_string(),
        ));
    }
    let mut dict_cursor = std::io::Cursor::new(&path_column.dictionary);
    let dict_count = dict_cursor.read_u32::<LittleEndian>()?;
    if dict_count != element_count {
        return Err(FormatError::InvalidMetadata(format!(
            "path dictionary count {dict_count} differs from file count {element_count}"
        )));
    }
    let mut paths = Vec::new();
    paths.try_reserve_exact(dict_count as usize).map_err(|_| {
        FormatError::ResourceLimitExceeded("path dictionary allocation failed".to_string())
    })?;
    for _ in 0..dict_count {
        let length = usize::try_from(read_varint(&mut dict_cursor)?).map_err(|_| {
            FormatError::ResourceLimitExceeded("path length exceeds address size".to_string())
        })?;
        if length > 4096 {
            return Err(FormatError::InvalidPath(
                "path exceeds 4096 UTF-8 bytes".to_string(),
            ));
        }
        let mut bytes = vec![0u8; length];
        dict_cursor.read_exact(&mut bytes)?;
        let path = String::from_utf8(bytes)
            .map_err(|_| FormatError::InvalidPath("dictionary path is not UTF-8".to_string()))?;
        validate_metadata_path(&path)?;
        if paths
            .last()
            .is_some_and(|previous: &String| previous.as_bytes() >= path.as_bytes())
        {
            return Err(FormatError::InvalidMetadata(
                "path dictionary must be unique and sorted by UTF-8 bytes".to_string(),
            ));
        }
        paths.push(path);
    }
    if dict_cursor.position() != path_column.dictionary.len() as u64 {
        return Err(FormatError::InvalidMetadata(
            "trailing bytes in path dictionary".to_string(),
        ));
    }

    let mut path_cursor = std::io::Cursor::new(&payloads[0]);
    let mut entry_paths = Vec::new();
    entry_paths
        .try_reserve_exact(element_count as usize)
        .map_err(|_| {
            FormatError::ResourceLimitExceeded("file table allocation failed".to_string())
        })?;
    for _ in 0..element_count {
        let index = usize::try_from(read_varint(&mut path_cursor)?).map_err(|_| {
            FormatError::InvalidMetadata("path ID exceeds address size".to_string())
        })?;
        let path = paths.get(index).ok_or_else(|| {
            FormatError::InvalidPath(format!("path dictionary index {index} is out of range"))
        })?;
        entry_paths.push(path.clone());
    }
    if path_cursor.position() != payloads[0].len() as u64 {
        return Err(FormatError::InvalidMetadata(
            "trailing bytes in path ID column".to_string(),
        ));
    }
    let mut unique_paths = std::collections::HashSet::new();
    if entry_paths.iter().any(|path| !unique_paths.insert(path)) {
        return Err(FormatError::PathCollision(
            "duplicate paths in metadata".to_string(),
        ));
    }

    let mut timestamp_cursor = std::io::Cursor::new(&payloads[1]);
    let mut timestamps = Vec::new();
    timestamps
        .try_reserve_exact(element_count as usize)
        .map_err(|_| {
            FormatError::ResourceLimitExceeded("timestamp table allocation failed".to_string())
        })?;
    let mut timestamp = 0i64;
    for index in 0..element_count {
        let delta = read_svarint(&mut timestamp_cursor)?;
        timestamp = if index == 0 {
            delta
        } else {
            timestamp.checked_add(delta).ok_or_else(|| {
                FormatError::IntegerOverflow("timestamp delta accumulation".to_string())
            })?
        };
        timestamps.push(timestamp);
    }
    if timestamp_cursor.position() != payloads[1].len() as u64 {
        return Err(FormatError::InvalidMetadata(
            "trailing bytes in timestamp column".to_string(),
        ));
    }

    let mut sizes_cursor = std::io::Cursor::new(&payloads[2]);
    let mut sizes = Vec::new();
    sizes
        .try_reserve_exact(element_count as usize)
        .map_err(|_| {
            FormatError::ResourceLimitExceeded("file size table allocation failed".to_string())
        })?;
    for _ in 0..element_count {
        sizes.push(read_varint(&mut sizes_cursor)?);
    }
    if sizes_cursor.position() != payloads[2].len() as u64 {
        return Err(FormatError::InvalidMetadata(
            "trailing bytes in file size column".to_string(),
        ));
    }

    let modes_size = (element_count as u64)
        .checked_mul(2)
        .ok_or_else(|| FormatError::IntegerOverflow("mode column size".to_string()))?;
    if modes_size != payloads[3].len() as u64 {
        return Err(FormatError::InvalidMetadata(
            "mode column has an invalid size".to_string(),
        ));
    }
    let mut modes = Vec::new();
    modes
        .try_reserve_exact(element_count as usize)
        .map_err(|_| {
            FormatError::ResourceLimitExceeded("mode table allocation failed".to_string())
        })?;
    for bytes in payloads[3].as_chunks::<2>().0 {
        let mode = u16::from_le_bytes(*bytes);
        if mode & 0xF000 != 0x8000 {
            return Err(FormatError::InvalidMetadata(format!(
                "mode 0o{mode:o} is not a regular file"
            )));
        }
        modes.push(mode);
    }

    let mut refs_cursor = std::io::Cursor::new(&payloads[4]);
    let mut chunk_refs = Vec::new();
    chunk_refs
        .try_reserve_exact(element_count as usize)
        .map_err(|_| {
            FormatError::ResourceLimitExceeded(
                "chunk reference table allocation failed".to_string(),
            )
        })?;
    for _ in 0..element_count {
        let count = usize::try_from(read_varint(&mut refs_cursor)?).map_err(|_| {
            FormatError::ResourceLimitExceeded(
                "chunk reference count exceeds address size".to_string(),
            )
        })?;
        if count as u64 > payloads[4].len() as u64 - refs_cursor.position() {
            return Err(FormatError::InvalidMetadata(
                "chunk reference count exceeds remaining payload".to_string(),
            ));
        }
        let mut refs = Vec::new();
        refs.try_reserve_exact(count).map_err(|_| {
            FormatError::ResourceLimitExceeded("chunk reference allocation failed".to_string())
        })?;
        for _ in 0..count {
            let index = u32::try_from(read_varint(&mut refs_cursor)?).map_err(|_| {
                FormatError::InvalidChunkIndex("chunk index exceeds u32".to_string())
            })?;
            if index >= chunk_index.count {
                return Err(FormatError::InvalidChunkIndex(format!(
                    "chunk reference {index} is outside the index"
                )));
            }
            refs.push(index);
        }
        chunk_refs.push(refs);
    }
    if refs_cursor.position() != payloads[4].len() as u64 {
        return Err(FormatError::InvalidMetadata(
            "trailing bytes in chunk reference column".to_string(),
        ));
    }

    let hashes_size = (element_count as u64)
        .checked_mul(32)
        .ok_or_else(|| FormatError::IntegerOverflow("content hash column size".to_string()))?;
    if hashes_size != payloads[5].len() as u64 {
        return Err(FormatError::InvalidMetadata(
            "content hash column has an invalid size".to_string(),
        ));
    }

    let entries = (0..element_count as usize)
        .map(|index| {
            let mut content_hash = [0u8; 32];
            content_hash.copy_from_slice(&payloads[5][index * 32..(index + 1) * 32]);
            ArchiveEntry {
                path: std::path::PathBuf::from(&entry_paths[index]),
                size: sizes[index],
                mtime: timestamps[index],
                mode: modes[index],
                content_hash,
                chunk_refs: std::mem::take(&mut chunk_refs[index]),
            }
        })
        .collect();
    Ok(entries)
}

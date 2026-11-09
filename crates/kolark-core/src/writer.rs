//! Archive writer

use kolark_codec::blake3::Blake3Hasher;
use kolark_codec::varint::{write_svarint as write_svarint_raw, write_varint as write_varint_raw};
use kolark_codec::{blake3_hash, cdc_split, lz_compress, CdcConfig};
use kolark_format::{
    ChunkIndex, ChunkIndexEntry, ChunkRecord, Column, ColumnCodec, ColumnEncoding, Footer,
    FormatError, FormatResult, Header, Superblock, CHUNK_FLAG_LZ, CHUNK_FLAG_RAW,
};
use std::collections::HashMap;
use std::fs::File;
use std::io::{Seek, Write};

fn write_varint<W: Write>(writer: &mut W, value: u64) -> FormatResult<()> {
    write_varint_raw(writer, value).map_err(|error| FormatError::InvalidVarint(error.to_string()))
}

fn write_svarint<W: Write>(writer: &mut W, value: i64) -> FormatResult<()> {
    write_svarint_raw(writer, value).map_err(|error| FormatError::InvalidVarint(error.to_string()))
}

#[derive(Debug, Clone)]
pub struct CreateOptions {
    pub force_overwrite: bool,
    pub chunk_size_min: u32,
    pub chunk_size_avg: u32,
    pub chunk_size_max: u32,
    pub deterministic: bool,
}

impl Default for CreateOptions {
    fn default() -> Self {
        Self {
            force_overwrite: false,
            chunk_size_min: 16384,
            chunk_size_avg: 65536,
            chunk_size_max: 262144,
            deterministic: true,
        }
    }
}

#[derive(Debug, Clone)]
pub struct InputEntry {
    pub path: std::path::PathBuf,
    pub mtime: Option<i64>,
    pub mode: Option<u16>,
}

impl Default for InputEntry {
    fn default() -> Self {
        Self {
            path: std::path::PathBuf::new(),
            mtime: None,
            mode: None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ArchiveSummary {
    pub file_count: u32,
    pub chunk_count: u32,
    pub archive_size: u64,
    pub dedup_ratio: f64,
}

struct ChunkStore {
    records: Vec<ChunkRecord>,
    raw_chunks: Vec<Vec<u8>>,
    hash_to_record: HashMap<[u8; 32], Vec<usize>>, // hash -> record indices
}

impl ChunkStore {
    fn new() -> Self {
        Self {
            records: Vec::new(),
            raw_chunks: Vec::new(),
            hash_to_record: HashMap::new(),
        }
    }

    fn add_chunk(&mut self, data: &[u8], config: &CdcConfig) -> FormatResult<Vec<u32>> {
        let chunks = cdc_split(data, *config);
        let mut refs = Vec::new();

        for chunk in chunks {
            let hash = blake3_hash(&chunk.data);
            let size_original = checked_u32_len(chunk.data.len(), "chunk size")?;

            // Check for existing chunk with same hash
            let mut found = false;
            if let Some(indices) = self.hash_to_record.get(&hash) {
                for &idx in indices {
                    if self.raw_chunks[idx] == chunk.data {
                        refs.push(idx as u32);
                        found = true;
                        break;
                    }
                }
            }

            if !found {
                // Compress with LZ
                let compressed = lz_compress(&chunk.data);
                let compressed_len = compressed.len();
                let flags = if compressed_len < chunk.data.len() {
                    CHUNK_FLAG_LZ
                } else {
                    CHUNK_FLAG_RAW
                };

                let (final_data, final_size) = if flags == CHUNK_FLAG_LZ {
                    (compressed, compressed_len)
                } else {
                    (chunk.data.clone(), chunk.data.len())
                };

                let record = ChunkRecord {
                    hash,
                    size_compressed: checked_u32_len(final_size, "compressed chunk size")?,
                    size_original,
                    flags,
                    data: final_data,
                };

                let idx = self.records.len();
                self.records.push(record);
                self.raw_chunks.push(chunk.data);
                self.hash_to_record.entry(hash).or_default().push(idx);
                refs.push(idx as u32);
            }
        }

        Ok(refs)
    }

    fn write<W: Write + Seek>(&self, writer: &mut W) -> FormatResult<u64> {
        let start_pos = writer.stream_position()?;
        for record in &self.records {
            record.write(writer)?;
        }
        Ok(writer.stream_position()? - start_pos)
    }
}

struct HashingWriter<W> {
    inner: W,
    hasher: Blake3Hasher,
}

fn validate_archive_path(path: &str) -> FormatResult<()> {
    let bytes = path.as_bytes();
    let components = path.split('/').collect::<Vec<_>>();
    let first = components.first().copied().unwrap_or_default();
    let scheme_like = first.split_once(':').is_some_and(|(scheme, _)| {
        !scheme.is_empty()
            && scheme.as_bytes()[0].is_ascii_alphabetic()
            && scheme
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'.' | b'-'))
    });

    if bytes.is_empty()
        || bytes.len() > 4096
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

fn checked_u32_len(size: usize, label: &str) -> FormatResult<u32> {
    u32::try_from(size)
        .map_err(|_| FormatError::ResourceLimitExceeded(format!("{label} exceeds u32::MAX")))
}

impl<W> HashingWriter<W> {
    fn new(inner: W) -> Self {
        Self {
            inner,
            hasher: Blake3Hasher::new(),
        }
    }

    fn body_hash(&self) -> [u8; 32] {
        self.hasher.clone().finalize()
    }
}

impl<W: Write> Write for HashingWriter<W> {
    fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
        let written = self.inner.write(buffer)?;
        self.hasher.update(&buffer[..written]);
        Ok(written)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

impl<W: Seek> Seek for HashingWriter<W> {
    fn seek(&mut self, position: std::io::SeekFrom) -> std::io::Result<u64> {
        self.inner.seek(position)
    }
}

pub fn create_archive<W: Write + Seek>(
    output: W,
    inputs: &[InputEntry],
    options: &CreateOptions,
) -> FormatResult<ArchiveSummary> {
    let mut output = output;
    if output.seek(std::io::SeekFrom::End(0))? != 0 {
        return Err(FormatError::InvalidOffset(
            "archive output must be empty".to_string(),
        ));
    }
    output.seek(std::io::SeekFrom::Start(0))?;
    let mut output = HashingWriter::new(output);

    let mut sorted_inputs = Vec::with_capacity(inputs.len());
    for input in inputs {
        let path = input
            .path
            .to_str()
            .ok_or_else(|| FormatError::InvalidPath("input path is not valid UTF-8".to_string()))?;
        validate_archive_path(path)?;
        sorted_inputs.push((path.as_bytes().to_vec(), input.clone()));
    }
    sorted_inputs.sort_by(|a, b| a.0.cmp(&b.0));

    for i in 1..sorted_inputs.len() {
        if sorted_inputs[i].0 == sorted_inputs[i - 1].0 {
            return Err(FormatError::PathCollision(format!(
                "duplicate path: {}",
                sorted_inputs[i].1.path.display()
            )));
        }
    }
    let sorted_inputs = sorted_inputs
        .into_iter()
        .map(|(_, input)| input)
        .collect::<Vec<_>>();

    let config = CdcConfig::new(
        options.chunk_size_min,
        options.chunk_size_avg,
        options.chunk_size_max,
    )
    .map_err(|error| FormatError::InvalidSuperblock(error.to_string()))?;

    let superblock = Superblock::with_params(
        options.chunk_size_min,
        options.chunk_size_avg,
        options.chunk_size_max,
    )?;

    let mut chunk_store = ChunkStore::new();
    let mut file_metadata = Vec::new();

    for input in &sorted_inputs {
        let path = input
            .path
            .to_str()
            .ok_or_else(|| FormatError::InvalidPath("input path is not valid UTF-8".to_string()))?
            .to_string();
        let metadata = std::fs::symlink_metadata(&input.path)?;
        if !metadata.file_type().is_file() {
            return Err(FormatError::UnsupportedFeature(format!(
                "only regular files are supported: {}",
                input.path.display()
            )));
        }
        let mut file = File::open(&input.path)?;
        let mut file_data = Vec::new();
        std::io::Read::read_to_end(&mut file, &mut file_data)?;

        let mtime = match input.mtime {
            Some(mtime) => mtime,
            None => {
                let modified = metadata.modified()?;
                match modified.duration_since(std::time::UNIX_EPOCH) {
                    Ok(duration) => i64::try_from(duration.as_secs()).map_err(|_| {
                        FormatError::InvalidMetadata("mtime exceeds i64 range".to_string())
                    })?,
                    Err(error) => i64::try_from(error.duration().as_secs())
                        .ok()
                        .and_then(i64::checked_neg)
                        .ok_or_else(|| {
                            FormatError::InvalidMetadata("mtime exceeds i64 range".to_string())
                        })?,
                }
            }
        };

        let mode = input.mode.unwrap_or_else(|| {
            #[cfg(unix)]
            {
                use std::os::unix::fs::MetadataExt;
                metadata.mode() as u16
            }
            #[cfg(not(unix))]
            {
                0o100644
            }
        });
        if mode & 0xF000 != 0x8000 {
            return Err(FormatError::InvalidMetadata(format!(
                "mode does not describe a regular file: 0o{mode:o}"
            )));
        }

        // Compute content hash
        let content_hash = blake3_hash(&file_data);

        // Split into chunks and add to store
        let chunk_refs = chunk_store.add_chunk(&file_data, &config)?;

        file_metadata.push(FileMeta {
            path,
            mtime,
            mode,
            size: file_data.len() as u64,
            content_hash,
            chunk_refs,
        });
    }

    let header = Header::new();
    header.write(&mut output)?;
    superblock.write(&mut output)?;
    let chunk_store_start = output.stream_position()?;
    if chunk_store_start != 48 {
        return Err(FormatError::InvalidOffset(format!(
            "chunk store must start at 48, got {chunk_store_start}"
        )));
    }
    chunk_store.write(&mut output)?;
    let table_meta_offset = output.stream_position()?;

    let (chunk_index, discovery_to_index) = build_chunk_index(&chunk_store)?;
    for file in &mut file_metadata {
        for reference in &mut file.chunk_refs {
            *reference = *discovery_to_index.get(*reference as usize).ok_or_else(|| {
                FormatError::InvalidChunkIndex("chunk reference is out of range".to_string())
            })?;
        }
    }

    let columns = build_columns(&file_metadata)?;
    for column in &columns {
        column.write(&mut output)?;
    }
    let chunk_index_offset = output.stream_position()?;
    chunk_index.write(&mut output)?;

    let hash_all = output.body_hash();
    let footer = Footer::new(table_meta_offset, chunk_index_offset, hash_all);
    footer.write(&mut output.inner)?;

    let archive_size = output.stream_position()?;
    output.flush()?;

    let total_input_bytes = file_metadata
        .iter()
        .try_fold(0u64, |total, file| total.checked_add(file.size))
        .ok_or_else(|| FormatError::IntegerOverflow("total input size".to_string()))?;
    let unique_chunk_bytes = chunk_store
        .raw_chunks
        .iter()
        .try_fold(0u64, |total, data| total.checked_add(data.len() as u64))
        .ok_or_else(|| FormatError::IntegerOverflow("unique chunk size".to_string()))?;
    let dedup_ratio = if total_input_bytes == 0 {
        0.0
    } else {
        1.0 - (unique_chunk_bytes as f64 / total_input_bytes as f64)
    };

    Ok(ArchiveSummary {
        file_count: u32::try_from(file_metadata.len())
            .map_err(|_| FormatError::ResourceLimitExceeded("too many files".to_string()))?,
        chunk_count: checked_u32_len(chunk_store.records.len(), "chunk count")?,
        archive_size,
        dedup_ratio,
    })
}

struct FileMeta {
    path: String,
    mtime: i64,
    mode: u16,
    size: u64,
    content_hash: [u8; 32],
    chunk_refs: Vec<u32>,
}

fn build_columns(files: &[FileMeta]) -> FormatResult<Vec<Column>> {
    let file_count = checked_u32_len(files.len(), "file count")?;
    let mut columns = Vec::with_capacity(6);

    let mut dict_entries = files
        .iter()
        .map(|file| file.path.clone())
        .collect::<Vec<_>>();
    dict_entries.sort_by(|left, right| left.as_bytes().cmp(right.as_bytes()));
    dict_entries.dedup();
    if dict_entries.len() != files.len() {
        return Err(FormatError::PathCollision(
            "duplicate file paths cannot be encoded".to_string(),
        ));
    }
    let path_to_idx = dict_entries
        .iter()
        .enumerate()
        .map(|(index, path)| (path.as_str(), index as u64))
        .collect::<HashMap<_, _>>();

    let mut dict_buf = Vec::new();
    dict_buf.extend_from_slice(&file_count.to_le_bytes());
    for path in &dict_entries {
        let path_bytes = path.as_bytes();
        write_varint(&mut dict_buf, path_bytes.len() as u64)?;
        dict_buf.extend_from_slice(path_bytes);
    }

    let mut indices_buf = Vec::new();
    for file in files {
        let idx = path_to_idx[file.path.as_str()];
        write_varint(&mut indices_buf, idx)?;
    }

    let compressed = lz_compress(&indices_buf);
    let compressed_len = checked_u32_len(compressed.len(), "compressed path index column")?;
    let original_indices_size = checked_u32_len(indices_buf.len(), "path index column")?;
    let (data, codec, compressed_size) = if compressed.len() < indices_buf.len() {
        (compressed, ColumnCodec::KolArkLZ, compressed_len)
    } else {
        (indices_buf, ColumnCodec::Raw, original_indices_size)
    };

    columns.push(Column {
        col_id: 0,
        encoding: ColumnEncoding::DictionaryVarint,
        codec,
        reserved: 0,
        element_count: file_count,
        dict_size: checked_u32_len(dict_buf.len(), "path dictionary")?,
        dictionary: dict_buf,
        compressed_size,
        original_size: original_indices_size,
        data,
    });

    // Column 1: timestamps (delta svarint)
    let mut ts_buf = Vec::new();
    let mut prev_ts: i64 = 0;
    for (i, file) in files.iter().enumerate() {
        let delta = if i == 0 {
            file.mtime
        } else {
            file.mtime
                .checked_sub(prev_ts)
                .ok_or_else(|| FormatError::IntegerOverflow("timestamp delta".to_string()))?
        };
        write_svarint(&mut ts_buf, delta)?;
        prev_ts = file.mtime;
    }

    let compressed = lz_compress(&ts_buf);
    let compressed_len = checked_u32_len(compressed.len(), "compressed timestamp column")?;
    let ts_size = checked_u32_len(ts_buf.len(), "timestamp column")?;
    let (data, codec, compressed_size) = if compressed.len() < ts_buf.len() {
        (compressed, ColumnCodec::KolArkLZ, compressed_len)
    } else {
        (ts_buf.clone(), ColumnCodec::Raw, ts_size)
    };

    columns.push(Column {
        col_id: 1,
        encoding: ColumnEncoding::DeltaSvarint,
        codec,
        reserved: 0,
        element_count: file_count,
        dict_size: 0,
        dictionary: Vec::new(),
        compressed_size,
        original_size: ts_size,
        data,
    });

    // Column 2: sizes (varint u64)
    let mut sizes_buf = Vec::new();
    for file in files {
        write_varint(&mut sizes_buf, file.size)?;
    }

    let compressed = lz_compress(&sizes_buf);
    let compressed_len = checked_u32_len(compressed.len(), "compressed file size column")?;
    let sizes_size = checked_u32_len(sizes_buf.len(), "file size column")?;
    let (data, codec, compressed_size) = if compressed.len() < sizes_buf.len() {
        (compressed, ColumnCodec::KolArkLZ, compressed_len)
    } else {
        (sizes_buf.clone(), ColumnCodec::Raw, sizes_size)
    };

    columns.push(Column {
        col_id: 2,
        encoding: ColumnEncoding::Varint,
        codec,
        reserved: 0,
        element_count: file_count,
        dict_size: 0,
        dictionary: Vec::new(),
        compressed_size,
        original_size: sizes_size,
        data,
    });

    // Column 3: modes (raw u16)
    let mut modes_buf = Vec::new();
    for file in files {
        modes_buf.extend_from_slice(&file.mode.to_le_bytes());
    }

    let compressed = lz_compress(&modes_buf);
    let compressed_len = checked_u32_len(compressed.len(), "compressed mode column")?;
    let modes_size = checked_u32_len(modes_buf.len(), "mode column")?;
    let (data, codec, compressed_size) = if compressed.len() < modes_buf.len() {
        (compressed, ColumnCodec::KolArkLZ, compressed_len)
    } else {
        (modes_buf.clone(), ColumnCodec::Raw, modes_size)
    };

    columns.push(Column {
        col_id: 3,
        encoding: ColumnEncoding::Raw,
        codec,
        reserved: 0,
        element_count: file_count,
        dict_size: 0,
        dictionary: Vec::new(),
        compressed_size,
        original_size: modes_size,
        data,
    });

    // Column 4: chunk_refs (varint count + varint indices)
    let mut refs_buf = Vec::new();
    for file in files {
        write_varint(&mut refs_buf, file.chunk_refs.len() as u64)?;
        for &ref_idx in &file.chunk_refs {
            write_varint(&mut refs_buf, ref_idx as u64)?;
        }
    }

    let compressed = lz_compress(&refs_buf);
    let compressed_len = checked_u32_len(compressed.len(), "compressed chunk reference column")?;
    let refs_size = checked_u32_len(refs_buf.len(), "chunk reference column")?;
    let (data, codec, compressed_size) = if compressed.len() < refs_buf.len() {
        (compressed, ColumnCodec::KolArkLZ, compressed_len)
    } else {
        (refs_buf.clone(), ColumnCodec::Raw, refs_size)
    };

    columns.push(Column {
        col_id: 4,
        encoding: ColumnEncoding::Varint,
        codec,
        reserved: 0,
        element_count: file_count,
        dict_size: 0,
        dictionary: Vec::new(),
        compressed_size,
        original_size: refs_size,
        data,
    });

    // Column 5: content_hash (raw 32 bytes each)
    let mut hash_buf = Vec::new();
    for file in files {
        hash_buf.extend_from_slice(&file.content_hash);
    }

    let compressed = lz_compress(&hash_buf);
    let compressed_len = checked_u32_len(compressed.len(), "compressed content hash column")?;
    let hashes_size = checked_u32_len(hash_buf.len(), "content hash column")?;
    let (data, codec, compressed_size) = if compressed.len() < hash_buf.len() {
        (compressed, ColumnCodec::KolArkLZ, compressed_len)
    } else {
        (hash_buf.clone(), ColumnCodec::Raw, hashes_size)
    };

    columns.push(Column {
        col_id: 5,
        encoding: ColumnEncoding::Raw,
        codec,
        reserved: 0,
        element_count: file_count,
        dict_size: 0,
        dictionary: Vec::new(),
        compressed_size,
        original_size: hashes_size,
        data,
    });

    Ok(columns)
}

fn build_chunk_index(chunk_store: &ChunkStore) -> FormatResult<(ChunkIndex, Vec<u32>)> {
    let mut offset = 48u64;
    let mut entries = Vec::with_capacity(chunk_store.records.len());
    for (discovery_index, record) in chunk_store.records.iter().enumerate() {
        entries.push((
            ChunkIndexEntry {
                hash: record.hash,
                offset,
            },
            discovery_index,
        ));
        offset = offset
            .checked_add(record.total_size())
            .ok_or_else(|| FormatError::IntegerOverflow("chunk store offset".to_string()))?;
    }

    entries.sort_by(|(left, _), (right, _)| {
        left.hash
            .cmp(&right.hash)
            .then_with(|| left.offset.cmp(&right.offset))
    });
    let count = checked_u32_len(entries.len(), "chunk index count")?;
    let mut discovery_to_index = vec![0u32; entries.len()];
    let sorted_entries = entries
        .into_iter()
        .enumerate()
        .map(|(index, (entry, discovery_index))| {
            discovery_to_index[discovery_index] = index as u32;
            entry
        })
        .collect();

    Ok((
        ChunkIndex {
            count,
            entries: sorted_entries,
        },
        discovery_to_index,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{open_archive, ReaderOptions, VerifyLevel};
    use std::io::Cursor;

    #[test]
    fn archive_round_trip_is_canonical_and_deduplicates() {
        let current_dir = std::env::current_dir().unwrap();
        let temp_dir = tempfile::Builder::new()
            .prefix("kolark-core-test-")
            .tempdir_in(&current_dir)
            .unwrap();
        let relative_dir = temp_dir.path().strip_prefix(&current_dir).unwrap();
        let data = b"KolArk archive round-trip test data with repeated content. ".repeat(20);
        std::fs::write(temp_dir.path().join("first.bin"), &data).unwrap();
        std::fs::write(temp_dir.path().join("second.bin"), &data).unwrap();
        std::fs::write(temp_dir.path().join("empty.bin"), []).unwrap();

        let inputs = ["first.bin", "second.bin", "empty.bin"].map(|name| InputEntry {
            path: relative_dir.join(name),
            ..InputEntry::default()
        });
        let options = CreateOptions {
            chunk_size_min: 16,
            chunk_size_avg: 32,
            chunk_size_max: 64,
            ..CreateOptions::default()
        };
        let mut archive = Cursor::new(Vec::new());
        let summary = create_archive(&mut archive, &inputs, &options).unwrap();
        assert_eq!(summary.file_count, 3);
        assert!(summary.chunk_count > 0);
        assert!(summary.dedup_ratio > 0.0);

        let bytes = archive.into_inner();
        let mut reader =
            open_archive(Cursor::new(bytes.clone()), &ReaderOptions::default()).unwrap();
        assert_eq!(reader.entries().len(), 3);
        assert!(reader.verify(VerifyLevel::Full).unwrap().valid);

        let mut second_archive = Cursor::new(Vec::new());
        create_archive(&mut second_archive, &inputs, &options).unwrap();
        assert_eq!(bytes, second_archive.into_inner());
    }

    #[test]
    fn empty_archive_is_valid() {
        let mut archive = Cursor::new(Vec::new());
        create_archive(
            &mut archive,
            &[],
            &CreateOptions {
                chunk_size_min: 1,
                chunk_size_avg: 2,
                chunk_size_max: 3,
                ..CreateOptions::default()
            },
        )
        .unwrap();
        let mut reader =
            open_archive(Cursor::new(archive.into_inner()), &ReaderOptions::default()).unwrap();
        assert!(reader.entries().is_empty());
        assert!(reader.verify(VerifyLevel::Full).unwrap().valid);
    }

    #[test]
    fn repeated_file_chunk_references_round_trip_through_lz() {
        let data = b"KolArk archive round-trip test data with repeated content. ".repeat(20);
        let config = CdcConfig::new(16, 32, 64).unwrap();
        let mut store = ChunkStore::new();
        let references = store.add_chunk(&data, &config).unwrap();
        let (_, discovery_to_index) = build_chunk_index(&store).unwrap();
        let final_references = references
            .iter()
            .map(|index| discovery_to_index[*index as usize])
            .collect::<Vec<_>>();
        let files = [
            FileMeta {
                path: "empty.bin".to_string(),
                mtime: 1,
                mode: 0o100644,
                size: 0,
                content_hash: blake3_hash(b""),
                chunk_refs: Vec::new(),
            },
            FileMeta {
                path: "first.bin".to_string(),
                mtime: 1,
                mode: 0o100644,
                size: data.len() as u64,
                content_hash: blake3_hash(&data),
                chunk_refs: final_references.clone(),
            },
            FileMeta {
                path: "second.bin".to_string(),
                mtime: 1,
                mode: 0o100644,
                size: data.len() as u64,
                content_hash: blake3_hash(&data),
                chunk_refs: final_references.clone(),
            },
        ];
        let columns = build_columns(&files).unwrap();
        let column = &columns[4];
        let decoded =
            kolark_codec::lz_decompress(&column.data, column.original_size as usize).unwrap();
        let mut expected = Vec::new();
        write_varint(&mut expected, 0).unwrap();
        for _ in 0..2 {
            write_varint(&mut expected, final_references.len() as u64).unwrap();
            for reference in &final_references {
                write_varint(&mut expected, *reference as u64).unwrap();
            }
        }
        assert_eq!(decoded, expected);
    }
}

//! Metadata column encoding and decoding.

use byteorder::ReadBytesExt;
use kolark_codec::lz::{lz_compress, lz_decompress};
use kolark_codec::varint::{
    read_svarint as read_svarint_raw, read_varint as read_varint_raw,
    write_svarint as write_svarint_raw, write_varint as write_varint_raw,
};
use kolark_format::{Column, ColumnCodec, ColumnEncoding, FormatError, FormatResult};
use std::io::{Cursor, Read};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ColumnData {
    pub paths: Vec<String>,
    pub timestamps: Vec<i64>,
    pub sizes: Vec<u64>,
    pub modes: Vec<u16>,
    pub chunk_refs: Vec<Vec<u32>>,
    pub content_hashes: Vec<[u8; 32]>,
}

fn write_varint<W: std::io::Write>(writer: &mut W, value: u64) -> FormatResult<()> {
    write_varint_raw(writer, value).map_err(|error| FormatError::InvalidVarint(error.to_string()))
}

fn write_svarint<W: std::io::Write>(writer: &mut W, value: i64) -> FormatResult<()> {
    write_svarint_raw(writer, value).map_err(|error| FormatError::InvalidVarint(error.to_string()))
}

fn read_varint<R: Read>(reader: &mut R) -> FormatResult<u64> {
    read_varint_raw(reader).map_err(|error| FormatError::InvalidVarint(error.to_string()))
}

fn read_svarint<R: Read>(reader: &mut R) -> FormatResult<i64> {
    read_svarint_raw(reader).map_err(|error| FormatError::InvalidVarint(error.to_string()))
}

fn checked_size(size: usize, label: &str) -> FormatResult<u32> {
    u32::try_from(size)
        .map_err(|_| FormatError::ResourceLimitExceeded(format!("{label} exceeds u32::MAX")))
}

fn validate_path(path: &str) -> FormatResult<()> {
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

fn make_column(
    col_id: u8,
    encoding: ColumnEncoding,
    element_count: u32,
    dictionary: Vec<u8>,
    original: Vec<u8>,
) -> FormatResult<Column> {
    let original_size = checked_size(original.len(), "column")?;
    let compressed = lz_compress(&original);
    let (data, codec) = if compressed.len() < original_size as usize {
        let compressed_size = checked_size(compressed.len(), "compressed column")?;
        (compressed, (ColumnCodec::KolArkLZ, compressed_size))
    } else {
        let raw_size = checked_size(original.len(), "column")?;
        (original, (ColumnCodec::Raw, raw_size))
    };

    Ok(Column {
        col_id,
        encoding,
        codec: codec.0,
        reserved: 0,
        element_count,
        dict_size: checked_size(dictionary.len(), "column dictionary")?,
        dictionary,
        compressed_size: codec.1,
        original_size,
        data,
    })
}

pub fn encode_columns(data: &ColumnData) -> FormatResult<Vec<Column>> {
    let file_count = checked_size(data.paths.len(), "file count")?;
    if data.timestamps.len() != data.paths.len()
        || data.sizes.len() != data.paths.len()
        || data.modes.len() != data.paths.len()
        || data.chunk_refs.len() != data.paths.len()
        || data.content_hashes.len() != data.paths.len()
    {
        return Err(FormatError::InvalidMetadata(
            "all metadata columns must have the same element count".to_string(),
        ));
    }

    let mut dictionary = data.paths.clone();
    for path in &dictionary {
        validate_path(path)?;
    }
    dictionary.sort_by(|left, right| left.as_bytes().cmp(right.as_bytes()));
    dictionary.dedup();
    if dictionary.len() != data.paths.len() {
        return Err(FormatError::PathCollision(
            "file paths must be unique".to_string(),
        ));
    }
    let path_ids = dictionary
        .iter()
        .enumerate()
        .map(|(index, path)| (path.as_str(), index as u64))
        .collect::<std::collections::HashMap<_, _>>();

    let mut dictionary_bytes = Vec::new();
    dictionary_bytes.extend_from_slice(&file_count.to_le_bytes());
    for path in &dictionary {
        write_varint(&mut dictionary_bytes, path.len() as u64)?;
        dictionary_bytes.extend_from_slice(path.as_bytes());
    }

    let mut path_ids_bytes = Vec::new();
    for path in &data.paths {
        write_varint(&mut path_ids_bytes, path_ids[path.as_str()])?;
    }

    let mut timestamps_bytes = Vec::new();
    let mut previous_timestamp = 0i64;
    for (index, &timestamp) in data.timestamps.iter().enumerate() {
        let delta = if index == 0 {
            timestamp
        } else {
            timestamp
                .checked_sub(previous_timestamp)
                .ok_or_else(|| FormatError::IntegerOverflow("timestamp delta".to_string()))?
        };
        write_svarint(&mut timestamps_bytes, delta)?;
        previous_timestamp = timestamp;
    }

    let mut sizes_bytes = Vec::new();
    for &size in &data.sizes {
        write_varint(&mut sizes_bytes, size)?;
    }

    let mut modes_bytes = Vec::new();
    for &mode in &data.modes {
        if mode & 0xF000 != 0x8000 {
            return Err(FormatError::InvalidMetadata(format!(
                "mode 0o{mode:o} is not a regular file"
            )));
        }
        modes_bytes.extend_from_slice(&mode.to_le_bytes());
    }

    let mut references_bytes = Vec::new();
    for references in &data.chunk_refs {
        write_varint(&mut references_bytes, references.len() as u64)?;
        for &reference in references {
            write_varint(&mut references_bytes, reference as u64)?;
        }
    }

    let mut hashes_bytes = Vec::new();
    for hash in &data.content_hashes {
        hashes_bytes.extend_from_slice(hash);
    }

    Ok(vec![
        make_column(
            0,
            ColumnEncoding::DictionaryVarint,
            file_count,
            dictionary_bytes,
            path_ids_bytes,
        )?,
        make_column(
            1,
            ColumnEncoding::DeltaSvarint,
            file_count,
            Vec::new(),
            timestamps_bytes,
        )?,
        make_column(
            2,
            ColumnEncoding::Varint,
            file_count,
            Vec::new(),
            sizes_bytes,
        )?,
        make_column(3, ColumnEncoding::Raw, file_count, Vec::new(), modes_bytes)?,
        make_column(
            4,
            ColumnEncoding::Varint,
            file_count,
            Vec::new(),
            references_bytes,
        )?,
        make_column(5, ColumnEncoding::Raw, file_count, Vec::new(), hashes_bytes)?,
    ])
}

fn decode_payload(column: &Column) -> FormatResult<Vec<u8>> {
    if column.data.len() != column.compressed_size as usize {
        return Err(FormatError::InvalidMetadata(format!(
            "column {} payload size mismatch",
            column.col_id
        )));
    }
    match column.codec {
        ColumnCodec::Raw if column.compressed_size == column.original_size => {
            Ok(column.data.clone())
        }
        ColumnCodec::Raw => Err(FormatError::InvalidMetadata(format!(
            "raw column {} has inconsistent sizes",
            column.col_id
        ))),
        ColumnCodec::KolArkLZ if column.compressed_size < column.original_size => {
            lz_decompress(&column.data, column.original_size as usize)
                .map_err(|error| FormatError::DecompressionError(error.to_string()))
        }
        ColumnCodec::KolArkLZ => Err(FormatError::InvalidMetadata(format!(
            "LZ column {} is not smaller than its original data",
            column.col_id
        ))),
    }
}

fn ensure_consumed<T>(cursor: &Cursor<T>, len: usize, label: &str) -> FormatResult<()> {
    if cursor.position() != len as u64 {
        return Err(FormatError::InvalidMetadata(format!(
            "trailing bytes in {label}"
        )));
    }
    Ok(())
}

pub fn decode_columns(columns: &[Column]) -> FormatResult<ColumnData> {
    if columns.len() != 6 {
        return Err(FormatError::InvalidMetadata(
            "expected 6 columns".to_string(),
        ));
    }
    let element_count = columns[0].element_count;
    let encodings = [
        ColumnEncoding::DictionaryVarint,
        ColumnEncoding::DeltaSvarint,
        ColumnEncoding::Varint,
        ColumnEncoding::Raw,
        ColumnEncoding::Varint,
        ColumnEncoding::Raw,
    ];
    let mut payloads = Vec::with_capacity(6);
    for (index, column) in columns.iter().enumerate() {
        if column.col_id as usize != index
            || column.element_count != element_count
            || column.encoding != encodings[index]
            || column.reserved != 0
        {
            return Err(FormatError::InvalidMetadata(format!(
                "invalid metadata column {}",
                index
            )));
        }
        if index != 0 && (column.dict_size != 0 || !column.dictionary.is_empty()) {
            return Err(FormatError::InvalidMetadata(format!(
                "column {} must not include a dictionary",
                index
            )));
        }
        if column.dictionary.len() != column.dict_size as usize {
            return Err(FormatError::InvalidMetadata(format!(
                "column {} dictionary size mismatch",
                index
            )));
        }
        payloads.push(decode_payload(column)?);
    }

    if columns[0].dictionary.len() < 4 {
        return Err(FormatError::InvalidMetadata(
            "path dictionary is shorter than its count field".to_string(),
        ));
    }
    let mut dictionary_cursor = Cursor::new(&columns[0].dictionary);
    let dictionary_count = dictionary_cursor.read_u32::<byteorder::LittleEndian>()?;
    if dictionary_count != element_count {
        return Err(FormatError::InvalidMetadata(
            "path dictionary count differs from file count".to_string(),
        ));
    }
    let mut paths = Vec::new();
    paths
        .try_reserve_exact(dictionary_count as usize)
        .map_err(|_| {
            FormatError::ResourceLimitExceeded("path dictionary allocation failed".to_string())
        })?;
    for _ in 0..dictionary_count {
        let length = usize::try_from(read_varint(&mut dictionary_cursor)?).map_err(|_| {
            FormatError::ResourceLimitExceeded("path length exceeds address size".to_string())
        })?;
        if length > 4096 {
            return Err(FormatError::InvalidPath(
                "path exceeds 4096 bytes".to_string(),
            ));
        }
        let mut bytes = vec![0u8; length];
        dictionary_cursor.read_exact(&mut bytes)?;
        let path = String::from_utf8(bytes)
            .map_err(|_| FormatError::InvalidPath("path is not valid UTF-8".to_string()))?;
        validate_path(&path)?;
        if paths
            .last()
            .is_some_and(|previous: &String| previous.as_bytes() >= path.as_bytes())
        {
            return Err(FormatError::InvalidMetadata(
                "path dictionary must be unique and sorted".to_string(),
            ));
        }
        paths.push(path);
    }
    ensure_consumed(
        &dictionary_cursor,
        columns[0].dictionary.len(),
        "path dictionary",
    )?;

    let mut path_ids_cursor = Cursor::new(&payloads[0]);
    let mut ordered_paths = Vec::new();
    ordered_paths
        .try_reserve_exact(element_count as usize)
        .map_err(|_| {
            FormatError::ResourceLimitExceeded("file table allocation failed".to_string())
        })?;
    for _ in 0..element_count {
        let path_id = usize::try_from(read_varint(&mut path_ids_cursor)?).map_err(|_| {
            FormatError::InvalidMetadata("path ID exceeds address size".to_string())
        })?;
        ordered_paths.push(
            paths
                .get(path_id)
                .ok_or_else(|| FormatError::InvalidPath("path ID is out of range".to_string()))?
                .clone(),
        );
    }
    ensure_consumed(&path_ids_cursor, payloads[0].len(), "path ID column")?;
    let mut unique_paths = std::collections::HashSet::new();
    if ordered_paths.iter().any(|path| !unique_paths.insert(path)) {
        return Err(FormatError::PathCollision(
            "duplicate file paths".to_string(),
        ));
    }

    let mut timestamps_cursor = Cursor::new(&payloads[1]);
    let mut timestamps = Vec::new();
    timestamps
        .try_reserve_exact(element_count as usize)
        .map_err(|_| {
            FormatError::ResourceLimitExceeded("timestamp allocation failed".to_string())
        })?;
    let mut timestamp = 0i64;
    for index in 0..element_count {
        let delta = read_svarint(&mut timestamps_cursor)?;
        timestamp = if index == 0 {
            delta
        } else {
            timestamp
                .checked_add(delta)
                .ok_or_else(|| FormatError::IntegerOverflow("timestamp accumulation".to_string()))?
        };
        timestamps.push(timestamp);
    }
    ensure_consumed(&timestamps_cursor, payloads[1].len(), "timestamp column")?;

    let mut sizes_cursor = Cursor::new(&payloads[2]);
    let mut sizes = Vec::new();
    sizes
        .try_reserve_exact(element_count as usize)
        .map_err(|_| {
            FormatError::ResourceLimitExceeded("file size allocation failed".to_string())
        })?;
    for _ in 0..element_count {
        sizes.push(read_varint(&mut sizes_cursor)?);
    }
    ensure_consumed(&sizes_cursor, payloads[2].len(), "file size column")?;

    let modes_size = (element_count as u64)
        .checked_mul(2)
        .ok_or_else(|| FormatError::IntegerOverflow("mode column size".to_string()))?;
    if modes_size != payloads[3].len() as u64 {
        return Err(FormatError::InvalidMetadata(
            "invalid mode column size".to_string(),
        ));
    }
    let mut modes = Vec::new();
    modes
        .try_reserve_exact(element_count as usize)
        .map_err(|_| FormatError::ResourceLimitExceeded("mode allocation failed".to_string()))?;
    for bytes in payloads[3].as_chunks::<2>().0 {
        let mode = u16::from_le_bytes(*bytes);
        if mode & 0xF000 != 0x8000 {
            return Err(FormatError::InvalidMetadata(
                "non-regular file mode".to_string(),
            ));
        }
        modes.push(mode);
    }

    let mut references_cursor = Cursor::new(&payloads[4]);
    let mut chunk_refs = Vec::new();
    chunk_refs
        .try_reserve_exact(element_count as usize)
        .map_err(|_| {
            FormatError::ResourceLimitExceeded("reference table allocation failed".to_string())
        })?;
    for _ in 0..element_count {
        let count = usize::try_from(read_varint(&mut references_cursor)?).map_err(|_| {
            FormatError::ResourceLimitExceeded("reference count exceeds address size".to_string())
        })?;
        if count as u64 > payloads[4].len() as u64 - references_cursor.position() {
            return Err(FormatError::InvalidMetadata(
                "reference count exceeds remaining payload".to_string(),
            ));
        }
        let mut references = Vec::new();
        references.try_reserve_exact(count).map_err(|_| {
            FormatError::ResourceLimitExceeded("chunk reference allocation failed".to_string())
        })?;
        for _ in 0..count {
            references.push(
                u32::try_from(read_varint(&mut references_cursor)?).map_err(|_| {
                    FormatError::InvalidChunkIndex("chunk reference exceeds u32".to_string())
                })?,
            );
        }
        chunk_refs.push(references);
    }
    ensure_consumed(
        &references_cursor,
        payloads[4].len(),
        "chunk reference column",
    )?;

    let hashes_size = (element_count as u64)
        .checked_mul(32)
        .ok_or_else(|| FormatError::IntegerOverflow("content hash size".to_string()))?;
    if hashes_size != payloads[5].len() as u64 {
        return Err(FormatError::InvalidMetadata(
            "invalid content hash column size".to_string(),
        ));
    }
    let content_hashes = payloads[5].as_chunks::<32>().0.to_vec();

    Ok(ColumnData {
        paths: ordered_paths,
        timestamps,
        sizes,
        modes,
        chunk_refs,
        content_hashes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metadata_round_trips_canonical_columns() {
        let data = ColumnData {
            paths: vec![
                "zeta.bin".to_string(),
                "alpha.bin".to_string(),
                "middle.bin".to_string(),
            ],
            timestamps: vec![i64::MIN + 1, 0, i64::MAX],
            sizes: vec![0, u64::MAX, 1],
            modes: vec![0o100644, 0o100600, 0o100644],
            chunk_refs: vec![Vec::new(), vec![0, 128, u32::MAX], vec![1]],
            content_hashes: vec![[0; 32], [0x5a; 32], [0xa5; 32]],
        };
        let columns = encode_columns(&data).unwrap();
        assert_eq!(
            u32::from_le_bytes(columns[0].dictionary[..4].try_into().unwrap()),
            3
        );
        assert_eq!(decode_columns(&columns).unwrap(), data);
    }

    #[test]
    fn empty_metadata_has_six_columns() {
        let data = ColumnData {
            paths: Vec::new(),
            timestamps: Vec::new(),
            sizes: Vec::new(),
            modes: Vec::new(),
            chunk_refs: Vec::new(),
            content_hashes: Vec::new(),
        };
        let columns = encode_columns(&data).unwrap();
        assert_eq!(columns.len(), 6);
        assert_eq!(decode_columns(&columns).unwrap(), data);
    }

    #[test]
    fn duplicate_paths_are_rejected() {
        let data = ColumnData {
            paths: vec!["same".to_string(), "same".to_string()],
            timestamps: vec![0, 0],
            sizes: vec![0, 0],
            modes: vec![0o100644, 0o100644],
            chunk_refs: vec![Vec::new(), Vec::new()],
            content_hashes: vec![[0; 32], [0; 32]],
        };
        assert!(matches!(
            encode_columns(&data),
            Err(FormatError::PathCollision(_))
        ));
    }
}

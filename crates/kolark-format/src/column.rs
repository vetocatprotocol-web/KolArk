//! Column structure for metadata

use byteorder::{LittleEndian, ReadBytesExt, WriteBytesExt};
use std::io::{Read, Seek, Write};

use crate::error::{FormatError, FormatResult};

/// Column fixed header size (20 bytes)
pub const COLUMN_FIXED_HEADER_SIZE: u32 = 20;

/// Column encoding types
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum ColumnEncoding {
    Raw = 0,
    Varint = 1,
    DeltaSvarint = 2,
    DictionaryVarint = 3,
}

impl ColumnEncoding {
    pub fn from_u8(v: u8) -> FormatResult<Self> {
        match v {
            0 => Ok(ColumnEncoding::Raw),
            1 => Ok(ColumnEncoding::Varint),
            2 => Ok(ColumnEncoding::DeltaSvarint),
            3 => Ok(ColumnEncoding::DictionaryVarint),
            _ => Err(FormatError::InvalidEncoding(format!(
                "invalid column encoding: {}",
                v
            ))),
        }
    }
}

/// Column codec types
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum ColumnCodec {
    Raw = 0,
    KolArkLZ = 1,
}

impl ColumnCodec {
    pub fn from_u8(v: u8) -> FormatResult<Self> {
        match v {
            0 => Ok(ColumnCodec::Raw),
            1 => Ok(ColumnCodec::KolArkLZ),
            _ => Err(FormatError::InvalidCodec(format!(
                "invalid column codec: {}",
                v
            ))),
        }
    }
}

/// Column record
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Column {
    pub col_id: u8,
    pub encoding: ColumnEncoding,
    pub codec: ColumnCodec,
    pub reserved: u8,
    pub element_count: u32,
    pub dict_size: u32,
    pub dictionary: Vec<u8>,
    pub compressed_size: u32,
    pub original_size: u32,
    pub data: Vec<u8>,
}

impl Column {
    /// Read a column whose bytes must fit within a known table range and memory budget.
    pub fn read_bounded<R: Read + Seek>(
        reader: &mut R,
        section_end: u64,
        max_allocation: u64,
    ) -> FormatResult<Self> {
        let start = reader.stream_position()?;
        let remaining = section_end.checked_sub(start).ok_or_else(|| {
            FormatError::InvalidOffset("column starts past metadata table end".to_string())
        })?;
        if remaining < COLUMN_FIXED_HEADER_SIZE as u64 {
            return Err(FormatError::InvalidMetadata(
                "truncated column header".to_string(),
            ));
        }

        let col_id = reader.read_u8()?;
        if col_id > 5 {
            return Err(FormatError::InvalidMetadata(format!(
                "invalid col_id: {col_id}, must be 0-5"
            )));
        }
        let encoding = ColumnEncoding::from_u8(reader.read_u8()?)?;
        let codec = ColumnCodec::from_u8(reader.read_u8()?)?;
        let reserved = reader.read_u8()?;
        let element_count = reader.read_u32::<LittleEndian>()?;
        let dict_size = reader.read_u32::<LittleEndian>()?;
        let fixed_and_dictionary = 20u64
            .checked_add(dict_size as u64)
            .ok_or_else(|| FormatError::IntegerOverflow("column dictionary end".to_string()))?;
        if fixed_and_dictionary > remaining {
            return Err(FormatError::InvalidMetadata(
                "column dictionary exceeds metadata table".to_string(),
            ));
        }
        if dict_size as u64 > max_allocation {
            return Err(FormatError::ResourceLimitExceeded(
                "column dictionary exceeds memory limit".to_string(),
            ));
        }

        let dictionary_size = dict_size as usize;
        let mut dictionary = Vec::new();
        dictionary.try_reserve_exact(dictionary_size).map_err(|_| {
            FormatError::ResourceLimitExceeded("column dictionary allocation failed".to_string())
        })?;
        dictionary.resize(dictionary_size, 0);
        reader.read_exact(&mut dictionary)?;

        let compressed_size = reader.read_u32::<LittleEndian>()?;
        let original_size = reader.read_u32::<LittleEndian>()?;
        let allocation_size = (dict_size as u64)
            .checked_add(compressed_size as u64)
            .ok_or_else(|| FormatError::IntegerOverflow("column allocation size".to_string()))?;
        let total_size = fixed_and_dictionary
            .checked_add(compressed_size as u64)
            .ok_or_else(|| FormatError::IntegerOverflow("column end".to_string()))?;
        if total_size > remaining {
            return Err(FormatError::InvalidMetadata(
                "column payload exceeds metadata table".to_string(),
            ));
        }
        if allocation_size > max_allocation {
            return Err(FormatError::ResourceLimitExceeded(
                "column exceeds memory limit".to_string(),
            ));
        }

        let data_size = compressed_size as usize;
        let mut data = Vec::new();
        data.try_reserve_exact(data_size).map_err(|_| {
            FormatError::ResourceLimitExceeded("column payload allocation failed".to_string())
        })?;
        data.resize(data_size, 0);
        reader.read_exact(&mut data)?;

        Ok(Self {
            col_id,
            encoding,
            codec,
            reserved,
            element_count,
            dict_size,
            dictionary,
            compressed_size,
            original_size,
            data,
        })
    }

    /// Read column from a reader
    pub fn read<R: Read>(reader: &mut R) -> FormatResult<Self> {
        let col_id = reader.read_u8()?;
        if col_id > 5 {
            return Err(FormatError::InvalidMetadata(format!(
                "invalid col_id: {}, must be 0-5",
                col_id
            )));
        }

        let encoding = ColumnEncoding::from_u8(reader.read_u8()?)?;
        let codec = ColumnCodec::from_u8(reader.read_u8()?)?;
        let reserved = reader.read_u8()?;
        let element_count = reader.read_u32::<LittleEndian>()?;
        let dict_size = reader.read_u32::<LittleEndian>()?;

        let dictionary_size = usize::try_from(dict_size).map_err(|_| {
            FormatError::ResourceLimitExceeded("column dictionary does not fit memory".to_string())
        })?;
        let mut dictionary = Vec::new();
        dictionary.try_reserve_exact(dictionary_size).map_err(|_| {
            FormatError::ResourceLimitExceeded("column dictionary allocation failed".to_string())
        })?;
        dictionary.resize(dictionary_size, 0);
        reader.read_exact(&mut dictionary)?;

        let compressed_size = reader.read_u32::<LittleEndian>()?;
        let original_size = reader.read_u32::<LittleEndian>()?;

        let data_size = usize::try_from(compressed_size).map_err(|_| {
            FormatError::ResourceLimitExceeded("column payload does not fit memory".to_string())
        })?;
        let mut data = Vec::new();
        data.try_reserve_exact(data_size).map_err(|_| {
            FormatError::ResourceLimitExceeded("column payload allocation failed".to_string())
        })?;
        data.resize(data_size, 0);
        reader.read_exact(&mut data)?;

        Ok(Self {
            col_id,
            encoding,
            codec,
            reserved,
            element_count,
            dict_size,
            dictionary,
            compressed_size,
            original_size,
            data,
        })
    }

    /// Write column to a writer
    pub fn write<W: Write>(&self, writer: &mut W) -> FormatResult<()> {
        writer.write_u8(self.col_id)?;
        writer.write_u8(self.encoding as u8)?;
        writer.write_u8(self.codec as u8)?;
        writer.write_u8(self.reserved)?;
        writer.write_u32::<LittleEndian>(self.element_count)?;
        writer.write_u32::<LittleEndian>(self.dict_size)?;
        writer.write_all(&self.dictionary)?;
        writer.write_u32::<LittleEndian>(self.compressed_size)?;
        writer.write_u32::<LittleEndian>(self.original_size)?;
        writer.write_all(&self.data)?;
        Ok(())
    }

    /// Get total column size
    pub fn total_size(&self) -> u64 {
        COLUMN_FIXED_HEADER_SIZE as u64 + self.dict_size as u64 + self.compressed_size as u64
    }

    /// Validate column
    pub fn validate(&self, expected_element_count: u32) -> FormatResult<()> {
        if self.col_id > 5 {
            return Err(FormatError::InvalidMetadata(format!(
                "invalid col_id: {}",
                self.col_id
            )));
        }
        if self.element_count != expected_element_count {
            return Err(FormatError::InvalidMetadata(format!(
                "element_count mismatch: expected {}, got {}",
                expected_element_count, self.element_count
            )));
        }

        // For columns other than 0, dict_size must be 0
        if self.col_id != 0 && self.dict_size != 0 {
            return Err(FormatError::InvalidMetadata(format!(
                "col_id {} must have dict_size 0",
                self.col_id
            )));
        }

        if self.codec == ColumnCodec::Raw && self.compressed_size != self.original_size {
            return Err(FormatError::InvalidMetadata(
                "raw column must have compressed_size == original_size".to_string(),
            ));
        }

        // Validate dictionary for column 0
        if self.col_id == 0 && self.dict_size > 0 {
            // Validate dictionary structure (count + strings)
            // This is done at a higher level
        }

        Ok(())
    }
}

/// Expected encoding for each column
pub fn expected_encoding(col_id: u8) -> ColumnEncoding {
    match col_id {
        0 => ColumnEncoding::DictionaryVarint, // path_ids
        1 => ColumnEncoding::DeltaSvarint,     // timestamps
        2 => ColumnEncoding::Varint,           // sizes
        3 => ColumnEncoding::Raw,              // modes
        4 => ColumnEncoding::Varint,           // chunk_refs
        5 => ColumnEncoding::Raw,              // content_hash
        _ => ColumnEncoding::Raw,
    }
}

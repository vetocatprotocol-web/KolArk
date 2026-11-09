//! Chunk record structure

use byteorder::{LittleEndian, ReadBytesExt, WriteBytesExt};
use std::io::{Read, Write};

use crate::error::{FormatError, FormatResult};

/// Chunk record header size (32 + 4 + 4 + 1 = 41 bytes)
pub const CHUNK_RECORD_HEADER_SIZE: u32 = 41;

/// Chunk flags
pub const CHUNK_FLAG_LZ: u8 = 0x00; // Compressed with KolArk-LZ
pub const CHUNK_FLAG_RAW: u8 = 0x01; // Raw/uncompressed

/// Chunk record
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChunkRecord {
    pub hash: [u8; 32],
    pub size_compressed: u32,
    pub size_original: u32,
    pub flags: u8,
    pub data: Vec<u8>,
}

impl ChunkRecord {
    /// Read chunk record from a reader
    pub fn read<R: Read>(reader: &mut R) -> FormatResult<Self> {
        let mut hash = [0u8; 32];
        reader.read_exact(&mut hash)?;

        let size_compressed = reader.read_u32::<LittleEndian>()?;
        let size_original = reader.read_u32::<LittleEndian>()?;
        let flags = reader.read_u8()?;

        // Validate flags
        if flags != CHUNK_FLAG_LZ && flags != CHUNK_FLAG_RAW {
            return Err(FormatError::InvalidEncoding(format!(
                "invalid chunk flags: 0x{:02x}, expected 0x00 (LZ) or 0x01 (raw)",
                flags
            )));
        }

        let size = usize::try_from(size_compressed).map_err(|_| {
            FormatError::ResourceLimitExceeded("chunk payload does not fit memory".to_string())
        })?;
        let mut data = Vec::new();
        data.try_reserve_exact(size).map_err(|_| {
            FormatError::ResourceLimitExceeded("chunk payload allocation failed".to_string())
        })?;
        data.resize(size, 0);
        reader.read_exact(&mut data)?;

        Ok(Self {
            hash,
            size_compressed,
            size_original,
            flags,
            data,
        })
    }

    /// Write chunk record to a writer
    pub fn write<W: Write>(&self, writer: &mut W) -> FormatResult<()> {
        writer.write_all(&self.hash)?;
        writer.write_u32::<LittleEndian>(self.size_compressed)?;
        writer.write_u32::<LittleEndian>(self.size_original)?;
        writer.write_u8(self.flags)?;
        writer.write_all(&self.data)?;
        Ok(())
    }

    /// Get total record size
    pub fn total_size(&self) -> u64 {
        CHUNK_RECORD_HEADER_SIZE as u64 + self.size_compressed as u64
    }

    /// Validate chunk record consistency
    pub fn validate(&self, chunk_size_max: u32) -> FormatResult<()> {
        if self.flags == CHUNK_FLAG_RAW && self.size_compressed != self.size_original {
            return Err(FormatError::InvalidEncoding(
                "raw chunk must have size_compressed == size_original".to_string(),
            ));
        }
        if self.size_original == 0 {
            return Err(FormatError::InvalidEncoding(
                "size_original must be > 0".to_string(),
            ));
        }
        if self.size_original > chunk_size_max {
            return Err(FormatError::InvalidEncoding(format!(
                "size_original {} exceeds chunk_size_max {}",
                self.size_original, chunk_size_max
            )));
        }
        Ok(())
    }
}

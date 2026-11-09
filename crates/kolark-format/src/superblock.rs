//! Superblock structure (32 bytes)

use byteorder::{LittleEndian, ReadBytesExt, WriteBytesExt};
use std::io::{Read, Write};

use crate::error::{FormatError, FormatResult};

/// Superblock size in bytes
pub const SUPERBLOCK_SIZE: u64 = 32;

/// Hash algorithm identifiers
pub const HASH_ALG_BLAKE3_256: u8 = 0;

/// Compression algorithm identifiers
pub const COMPRESSION_KOLARK_LZ: u8 = 0;

/// Superblock (32 bytes)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Superblock {
    pub chunk_size_min: u32,
    pub chunk_size_avg: u32,
    pub chunk_size_max: u32,
    pub hash_alg: u8,
    pub compression: u8,
    pub flags: u16,
    pub reserved: [u8; 16],
}

impl Superblock {
    /// Create a new superblock with recommended defaults
    pub fn new() -> Self {
        Self {
            chunk_size_min: 16384,
            chunk_size_avg: 65536,
            chunk_size_max: 262144,
            hash_alg: HASH_ALG_BLAKE3_256,
            compression: COMPRESSION_KOLARK_LZ,
            flags: 0,
            reserved: [0; 16],
        }
    }

    /// Create a new superblock with custom CDC parameters
    pub fn with_params(
        chunk_size_min: u32,
        chunk_size_avg: u32,
        chunk_size_max: u32,
    ) -> FormatResult<Self> {
        if chunk_size_min < 1 {
            return Err(FormatError::InvalidSuperblock(
                "chunk_size_min must be >= 1".to_string(),
            ));
        }
        if !(chunk_size_min < chunk_size_avg && chunk_size_avg < chunk_size_max) {
            return Err(FormatError::InvalidSuperblock(
                "must have chunk_size_min < chunk_size_avg < chunk_size_max".to_string(),
            ));
        }

        Ok(Self {
            chunk_size_min,
            chunk_size_avg,
            chunk_size_max,
            hash_alg: HASH_ALG_BLAKE3_256,
            compression: COMPRESSION_KOLARK_LZ,
            flags: 0,
            reserved: [0; 16],
        })
    }

    /// Read superblock from a reader
    pub fn read<R: Read>(reader: &mut R) -> FormatResult<Self> {
        let chunk_size_min = reader.read_u32::<LittleEndian>()?;
        let chunk_size_avg = reader.read_u32::<LittleEndian>()?;
        let chunk_size_max = reader.read_u32::<LittleEndian>()?;
        let hash_alg = reader.read_u8()?;
        let compression = reader.read_u8()?;
        let flags = reader.read_u16::<LittleEndian>()?;
        let mut reserved = [0u8; 16];
        reader.read_exact(&mut reserved)?;

        Ok(Self {
            chunk_size_min,
            chunk_size_avg,
            chunk_size_max,
            hash_alg,
            compression,
            flags,
            reserved,
        })
    }

    /// Write superblock to a writer
    pub fn write<W: Write>(&self, writer: &mut W) -> FormatResult<()> {
        writer.write_u32::<LittleEndian>(self.chunk_size_min)?;
        writer.write_u32::<LittleEndian>(self.chunk_size_avg)?;
        writer.write_u32::<LittleEndian>(self.chunk_size_max)?;
        writer.write_u8(self.hash_alg)?;
        writer.write_u8(self.compression)?;
        writer.write_u16::<LittleEndian>(self.flags)?;
        writer.write_all(&self.reserved)?;
        Ok(())
    }

    /// Validate superblock
    pub fn validate(&self) -> FormatResult<()> {
        if self.chunk_size_min < 1 {
            return Err(FormatError::InvalidSuperblock(
                "chunk_size_min must be >= 1".to_string(),
            ));
        }
        if !(self.chunk_size_min < self.chunk_size_avg && self.chunk_size_avg < self.chunk_size_max)
        {
            return Err(FormatError::InvalidSuperblock(
                "must have chunk_size_min < chunk_size_avg < chunk_size_max".to_string(),
            ));
        }
        if self.hash_alg != HASH_ALG_BLAKE3_256 {
            return Err(FormatError::InvalidSuperblock(format!(
                "unsupported hash_alg: {}",
                self.hash_alg
            )));
        }
        if self.compression != COMPRESSION_KOLARK_LZ {
            return Err(FormatError::InvalidSuperblock(format!(
                "unsupported compression: {}",
                self.compression
            )));
        }
        if self.flags != 0 {
            return Err(FormatError::InvalidSuperblock(format!(
                "flags must be 0, got {}",
                self.flags
            )));
        }
        Ok(())
    }

    /// Calculate boundary span for CDC
    pub fn boundary_span(&self) -> u32 {
        self.chunk_size_avg - self.chunk_size_min
    }
}

impl Default for Superblock {
    fn default() -> Self {
        Self::new()
    }
}

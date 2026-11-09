//! Header structure (16 bytes)

use byteorder::{LittleEndian, ReadBytesExt, WriteBytesExt};
use std::io::{Read, Write};

use crate::error::{FormatError, FormatResult};

/// Magic bytes: KOLA
pub const MAGIC: [u8; 4] = *b"KOLA";
/// Header size in bytes
pub const HEADER_SIZE: u64 = 16;
/// Format version
pub const VERSION: u32 = 1;

/// Archive header (16 bytes)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Header {
    pub magic: [u8; 4],
    pub version: u32,
    pub flags: u32,
    pub reserved: u32,
}

impl Header {
    /// Create a new valid header
    pub fn new() -> Self {
        Self {
            magic: MAGIC,
            version: VERSION,
            flags: 0,
            reserved: 0,
        }
    }

    /// Read header from a reader
    pub fn read<R: Read>(reader: &mut R) -> FormatResult<Self> {
        let mut magic = [0u8; 4];
        reader.read_exact(&mut magic)?;

        let version = reader.read_u32::<LittleEndian>()?;
        let flags = reader.read_u32::<LittleEndian>()?;
        let reserved = reader.read_u32::<LittleEndian>()?;

        Ok(Self {
            magic,
            version,
            flags,
            reserved,
        })
    }

    /// Write header to a writer
    pub fn write<W: Write>(&self, writer: &mut W) -> FormatResult<()> {
        writer.write_all(&self.magic)?;
        writer.write_u32::<LittleEndian>(self.version)?;
        writer.write_u32::<LittleEndian>(self.flags)?;
        writer.write_u32::<LittleEndian>(self.reserved)?;
        Ok(())
    }

    /// Validate header
    pub fn validate(&self) -> FormatResult<()> {
        if self.magic != MAGIC {
            return Err(FormatError::InvalidMagic(self.magic));
        }
        if self.version != VERSION {
            return Err(FormatError::UnsupportedVersion(self.version));
        }
        if self.flags != 0 {
            return Err(FormatError::UnsupportedFlags(self.flags));
        }
        Ok(())
    }
}

impl Default for Header {
    fn default() -> Self {
        Self::new()
    }
}

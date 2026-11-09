//! Footer structure (72 bytes)

use byteorder::{LittleEndian, ReadBytesExt, WriteBytesExt};
use std::io::{Read, Write};

use crate::error::{FormatError, FormatResult};

/// Footer size in bytes
pub const FOOTER_SIZE: u64 = 72;
/// Footer magic
pub const FOOTER_MAGIC: [u8; 4] = *b"KOLA";
/// Expected footer length
pub const FOOTER_LEN: u32 = 72;

/// Archive footer (72 bytes)
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Footer {
    pub offset_superblock: u64,
    pub offset_chunk_store: u64,
    pub offset_table_meta: u64,
    pub offset_chunk_index: u64,
    pub hash_all: [u8; 32],
    pub footer_len: u32,
    pub magic: [u8; 4],
}

impl Footer {
    /// Create a new footer
    pub fn new(offset_table_meta: u64, offset_chunk_index: u64, hash_all: [u8; 32]) -> Self {
        Self {
            offset_superblock: 16,
            offset_chunk_store: 48,
            offset_table_meta,
            offset_chunk_index,
            hash_all,
            footer_len: FOOTER_LEN,
            magic: FOOTER_MAGIC,
        }
    }

    /// Read footer from a reader at the end of file
    pub fn read<R: Read + std::io::Seek>(reader: &mut R, file_size: u64) -> FormatResult<Self> {
        if file_size < FOOTER_SIZE + 48 {
            // minimum file size: header + superblock + footer
            return Err(FormatError::InvalidFooter(format!(
                "file too small: {} bytes, minimum {}",
                file_size,
                FOOTER_SIZE + 48
            )));
        }

        let footer_start = file_size - FOOTER_SIZE;
        reader.seek(std::io::SeekFrom::Start(footer_start))?;

        // Read and verify magic at the end
        let mut magic_check = [0u8; 4];
        reader.seek(std::io::SeekFrom::Start(file_size - 4))?;
        reader.read_exact(&mut magic_check)?;
        if magic_check != FOOTER_MAGIC {
            return Err(FormatError::InvalidMagic(magic_check));
        }

        // Read footer_len (4 bytes before magic)
        reader.seek(std::io::SeekFrom::Start(file_size - 8))?;
        let footer_len = reader.read_u32::<LittleEndian>()?;
        if footer_len != FOOTER_LEN {
            return Err(FormatError::InvalidFooter(format!(
                "invalid footer_len: {}, expected {}",
                footer_len, FOOTER_LEN
            )));
        }

        // Read full footer
        reader.seek(std::io::SeekFrom::Start(footer_start))?;

        let offset_superblock = reader.read_u64::<LittleEndian>()?;
        let offset_chunk_store = reader.read_u64::<LittleEndian>()?;
        let offset_table_meta = reader.read_u64::<LittleEndian>()?;
        let offset_chunk_index = reader.read_u64::<LittleEndian>()?;
        let mut hash_all = [0u8; 32];
        reader.read_exact(&mut hash_all)?;
        let footer_len_read = reader.read_u32::<LittleEndian>()?;
        let mut magic = [0u8; 4];
        reader.read_exact(&mut magic)?;

        if footer_len_read != FOOTER_LEN {
            return Err(FormatError::InvalidFooter(format!(
                "footer_len mismatch: {}, expected {}",
                footer_len_read, FOOTER_LEN
            )));
        }
        if magic != FOOTER_MAGIC {
            return Err(FormatError::InvalidMagic(magic));
        }

        let footer = Self {
            offset_superblock,
            offset_chunk_store,
            offset_table_meta,
            offset_chunk_index,
            hash_all,
            footer_len: footer_len_read,
            magic,
        };

        footer.validate()?;
        Ok(footer)
    }

    /// Write footer to a writer
    pub fn write<W: Write>(&self, writer: &mut W) -> FormatResult<()> {
        writer.write_u64::<LittleEndian>(self.offset_superblock)?;
        writer.write_u64::<LittleEndian>(self.offset_chunk_store)?;
        writer.write_u64::<LittleEndian>(self.offset_table_meta)?;
        writer.write_u64::<LittleEndian>(self.offset_chunk_index)?;
        writer.write_all(&self.hash_all)?;
        writer.write_u32::<LittleEndian>(self.footer_len)?;
        writer.write_all(&self.magic)?;
        Ok(())
    }

    /// Validate footer fields
    pub fn validate(&self) -> FormatResult<()> {
        if self.offset_superblock != 16 {
            return Err(FormatError::InvalidFooter(format!(
                "offset_superblock must be 16, got {}",
                self.offset_superblock
            )));
        }
        if self.offset_chunk_store != 48 {
            return Err(FormatError::InvalidFooter(format!(
                "offset_chunk_store must be 48, got {}",
                self.offset_chunk_store
            )));
        }
        if self.footer_len != FOOTER_LEN {
            return Err(FormatError::InvalidFooter(format!(
                "footer_len must be {}, got {}",
                FOOTER_LEN, self.footer_len
            )));
        }
        if self.magic != FOOTER_MAGIC {
            return Err(FormatError::InvalidMagic(self.magic));
        }

        // Validate offset ordering: 48 <= offset_table_meta < offset_chunk_index < footer_start
        if self.offset_table_meta < 48 {
            return Err(FormatError::InvalidFooter(format!(
                "offset_table_meta must be >= 48, got {}",
                self.offset_table_meta
            )));
        }
        if self.offset_table_meta >= self.offset_chunk_index {
            return Err(FormatError::InvalidFooter(
                "offset_table_meta must be < offset_chunk_index".to_string(),
            ));
        }
        // footer_start will be validated by reader based on file size

        Ok(())
    }

    /// Validate footer against file size
    pub fn validate_file_size(&self, file_size: u64) -> FormatResult<()> {
        let footer_start = file_size - FOOTER_SIZE;
        if self.offset_chunk_index >= footer_start {
            return Err(FormatError::InvalidFooter(format!(
                "offset_chunk_index {} >= footer_start {}",
                self.offset_chunk_index, footer_start
            )));
        }
        Ok(())
    }
}

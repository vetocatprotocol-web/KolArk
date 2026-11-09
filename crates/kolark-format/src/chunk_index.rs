//! Chunk Index structure

use byteorder::{LittleEndian, ReadBytesExt, WriteBytesExt};
use std::collections::HashSet;
use std::io::{Read, Write};

use crate::error::{FormatError, FormatResult};

/// Chunk index entry size (32 + 8 = 40 bytes)
pub const CHUNK_INDEX_ENTRY_SIZE: u32 = 40;

/// Chunk index entry
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChunkIndexEntry {
    pub hash: [u8; 32],
    pub offset: u64,
}

impl ChunkIndexEntry {
    /// Read chunk index entry from a reader
    pub fn read<R: Read>(reader: &mut R) -> FormatResult<Self> {
        let mut hash = [0u8; 32];
        reader.read_exact(&mut hash)?;
        let offset = reader.read_u64::<LittleEndian>()?;
        Ok(Self { hash, offset })
    }

    /// Write chunk index entry to a writer
    pub fn write<W: Write>(&self, writer: &mut W) -> FormatResult<()> {
        writer.write_all(&self.hash)?;
        writer.write_u64::<LittleEndian>(self.offset)?;
        Ok(())
    }
}

/// Chunk Index
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChunkIndex {
    pub count: u32,
    pub entries: Vec<ChunkIndexEntry>,
}

impl ChunkIndex {
    /// Read chunk index from a reader
    pub fn read<R: Read>(reader: &mut R) -> FormatResult<Self> {
        Self::read_bounded(reader, u32::MAX)
    }

    /// Read a chunk index while enforcing an implementation-defined entry limit.
    pub fn read_bounded<R: Read>(reader: &mut R, max_entries: u32) -> FormatResult<Self> {
        let count = reader.read_u32::<LittleEndian>()?;
        if count > max_entries {
            return Err(FormatError::ResourceLimitExceeded(format!(
                "chunk count {count} exceeds configured limit {max_entries}"
            )));
        }
        let mut entries = Vec::new();
        entries.try_reserve_exact(count as usize).map_err(|_| {
            FormatError::ResourceLimitExceeded("chunk index allocation failed".to_string())
        })?;
        for _ in 0..count {
            entries.push(ChunkIndexEntry::read(reader)?);
        }
        Ok(Self { count, entries })
    }

    /// Write chunk index to a writer
    pub fn write<W: Write>(&self, writer: &mut W) -> FormatResult<()> {
        writer.write_u32::<LittleEndian>(self.count)?;
        for entry in &self.entries {
            entry.write(writer)?;
        }
        Ok(())
    }

    /// Get total size in bytes
    pub fn total_size(&self) -> u64 {
        4 + (self.count as u64 * CHUNK_INDEX_ENTRY_SIZE as u64)
    }

    /// Validate chunk index
    pub fn validate(&self, chunk_store_end: u64) -> FormatResult<()> {
        if self.count as usize != self.entries.len() {
            return Err(FormatError::InvalidChunkIndex(
                "count doesn't match entries length".to_string(),
            ));
        }

        if self.count == 0 {
            if chunk_store_end != 48 {
                return Err(FormatError::InvalidChunkIndex(format!(
                    "empty chunk index but chunk_store_end is {} not 48",
                    chunk_store_end
                )));
            }
            return Ok(());
        }

        // Check ordering: lexicographic by hash, then by offset ascending
        for i in 1..self.entries.len() {
            let prev = &self.entries[i - 1];
            let curr = &self.entries[i];
            if curr.hash < prev.hash {
                return Err(FormatError::InvalidChunkIndex(format!(
                    "entries not sorted by hash at index {}",
                    i
                )));
            }
            if curr.hash == prev.hash && curr.offset < prev.offset {
                return Err(FormatError::InvalidChunkIndex(format!(
                    "entries with same hash not sorted by offset at index {}",
                    i
                )));
            }
        }

        let mut offsets = HashSet::with_capacity(self.entries.len());
        for (i, entry) in self.entries.iter().enumerate() {
            if entry.offset < 48 || entry.offset >= chunk_store_end {
                return Err(FormatError::InvalidChunkIndex(format!(
                    "entry {} offset {} out of range [48, {})",
                    i, entry.offset, chunk_store_end
                )));
            }
            if !offsets.insert(entry.offset) {
                return Err(FormatError::InvalidChunkIndex(format!(
                    "multiple index entries point to offset {}",
                    entry.offset
                )));
            }
        }

        Ok(())
    }

    /// Sort entries canonically: by hash lexicographically, then by offset ascending
    pub fn sort_canonical(&mut self) {
        self.entries
            .sort_by(|a, b| a.hash.cmp(&b.hash).then_with(|| a.offset.cmp(&b.offset)));
    }

    /// Find entry index by hash and offset (for collision handling)
    pub fn find_entry(&self, hash: &[u8; 32], offset: u64) -> Option<usize> {
        self.entries
            .iter()
            .position(|e| e.hash == *hash && e.offset == offset)
    }

    /// Find all entries with a given hash (for collision handling)
    pub fn find_by_hash(&self, hash: &[u8; 32]) -> Vec<usize> {
        self.entries
            .iter()
            .enumerate()
            .filter(|(_, e)| e.hash == *hash)
            .map(|(i, _)| i)
            .collect()
    }
}

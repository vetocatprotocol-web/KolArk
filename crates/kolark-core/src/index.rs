//! Chunk index building and validation

use kolark_format::{ChunkIndex, ChunkIndexEntry, ChunkRecord, FormatError, FormatResult};
use std::collections::HashMap;

pub fn build_chunk_index(records: &[ChunkRecord]) -> FormatResult<ChunkIndex> {
    let mut entries = Vec::with_capacity(records.len());
    let mut offset = 48u64; // Chunk store starts at offset 48

    for record in records {
        entries.push(ChunkIndexEntry {
            hash: record.hash,
            offset,
        });
        offset += record.total_size();
    }

    // Sort canonically: by hash lexicographically, then by offset ascending
    entries.sort_by(|a, b| a.hash.cmp(&b.hash).then_with(|| a.offset.cmp(&b.offset)));

    Ok(ChunkIndex {
        count: entries.len() as u32,
        entries,
    })
}

pub fn validate_chunk_index(index: &ChunkIndex, chunk_store_end: u64) -> FormatResult<()> {
    if index.count as usize != index.entries.len() {
        return Err(FormatError::InvalidChunkIndex(
            "count doesn't match entries length".to_string(),
        ));
    }

    if index.count == 0 {
        if chunk_store_end != 48 {
            return Err(FormatError::InvalidChunkIndex(format!(
                "empty chunk index but chunk_store_end is {} not 48",
                chunk_store_end
            )));
        }
        return Ok(());
    }

    // Check ordering: lexicographic by hash, then by offset ascending
    for i in 1..index.entries.len() {
        let prev = &index.entries[i - 1];
        let curr = &index.entries[i];
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

    // Validate offsets
    let mut prev_end = 48u64;
    for (i, entry) in index.entries.iter().enumerate() {
        if entry.offset < 48 || entry.offset >= chunk_store_end {
            return Err(FormatError::InvalidChunkIndex(format!(
                "entry {} offset {} out of range [48, {})",
                i, entry.offset, chunk_store_end
            )));
        }
        if entry.offset != prev_end {
            return Err(FormatError::InvalidChunkIndex(format!(
                "entry {} offset {} doesn't match expected {} (overlap/gap)",
                i, entry.offset, prev_end
            )));
        }
        prev_end = entry.offset;
    }

    Ok(())
}

pub fn map_chunk_refs_to_sorted_index(
    original_refs: &[u32],
    original_records: &[ChunkRecord],
    sorted_index: &ChunkIndex,
) -> Vec<u32> {
    // Build a map from (hash, original_offset) to sorted index
    let mut original_offsets = Vec::with_capacity(original_records.len());
    let mut offset = 48u64;
    for record in original_records {
        original_offsets.push(offset);
        offset += record.total_size();
    }

    let mut map = HashMap::new();
    for (sorted_idx, entry) in sorted_index.entries.iter().enumerate() {
        // Find which original record this corresponds to
        for (orig_idx, record) in original_records.iter().enumerate() {
            if record.hash == entry.hash && original_offsets[orig_idx] == entry.offset {
                map.insert(orig_idx, sorted_idx);
                break;
            }
        }
    }

    original_refs
        .iter()
        .map(|&r| map[&(r as usize)] as u32)
        .collect()
}

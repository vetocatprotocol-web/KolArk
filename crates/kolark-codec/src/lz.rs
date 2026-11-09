//! KolArk-LZ compression (LZ77 variant)

use crate::varint::{read_varint_from_slice, VarintError};

/// LZ compression/decompression errors
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum LzError {
    #[error("invalid offset: 0")]
    InvalidOffsetZero,
    #[error("offset {0} exceeds available output ({1})")]
    OffsetExceedsOutput(u16, usize),
    #[error("varint error: {0}")]
    Varint(#[from] VarintError),
    #[error("output would exceed declared original_size: {0} > {1}")]
    OutputExceedsLimit(usize, usize),
    #[error("trailing bytes after stream end")]
    TrailingBytes,
    #[error("unexpected EOF")]
    UnexpectedEof,
    #[error("match length too small: {0} < 4")]
    MatchTooSmall(usize),
    #[error("length does not fit the platform address size")]
    LengthOverflow,
    #[error("unable to reserve memory for decompressed output")]
    AllocationFailed,
}

/// Compress data using canonical KolArk-LZ encoder
pub fn lz_compress(data: &[u8]) -> Vec<u8> {
    if data.is_empty() {
        return Vec::new();
    }

    let mut output = Vec::new();
    let mut positions = std::collections::HashMap::<[u8; 4], Vec<usize>>::new();
    let mut pos = 0;
    let mut last_match_end = 0;

    while pos < data.len() {
        let (best_offset, best_len) = find_best_match(data, pos, &positions);

        if best_len >= 4 {
            emit_literal(&mut output, &data[last_match_end..pos]);
            emit_match(&mut output, best_offset, best_len);

            let match_end = pos + best_len;
            add_positions(data, pos, match_end, &mut positions);
            pos = match_end;
            last_match_end = pos;
        } else {
            add_positions(data, pos, pos + 1, &mut positions);
            pos += 1;
        }
    }

    // Emit final literals
    if last_match_end < data.len() {
        emit_literal(&mut output, &data[last_match_end..]);
    }

    output
}

/// Find the best match at position `pos` in `data`
/// Returns (offset, length) where offset is 1-based distance back
fn find_best_match(
    data: &[u8],
    pos: usize,
    positions: &std::collections::HashMap<[u8; 4], Vec<usize>>,
) -> (u16, usize) {
    if pos + 4 > data.len() {
        return (0, 0);
    }

    let key = [data[pos], data[pos + 1], data[pos + 2], data[pos + 3]];
    let mut best_len = 0;
    let mut best_offset = u16::MAX;

    if let Some(candidates) = positions.get(&key) {
        for &candidate in candidates.iter().rev() {
            let offset = pos - candidate;
            if offset > u16::MAX as usize {
                break;
            }

            let mut len = 0;
            while pos + len < data.len() && data[candidate + len] == data[pos + len] {
                len += 1;
            }

            if len >= 4 && len > best_len {
                best_len = len;
                best_offset = offset as u16;
            }

            if best_len == data.len() - pos {
                break;
            }
        }
    }

    if best_len >= 4 {
        (best_offset, best_len)
    } else {
        (0, 0)
    }
}

fn add_positions(
    data: &[u8],
    start: usize,
    end: usize,
    positions: &mut std::collections::HashMap<[u8; 4], Vec<usize>>,
) {
    for index in start..end.min(data.len().saturating_sub(3)) {
        let key = [
            data[index],
            data[index + 1],
            data[index + 2],
            data[index + 3],
        ];
        positions.entry(key).or_default().push(index);
    }
}

/// Emit a literal token
fn emit_literal(output: &mut Vec<u8>, data: &[u8]) {
    let len = data.len();
    if len <= 254 {
        output.push(len as u8);
    } else {
        output.push(0xFF);
        append_varint(output, (len - 255) as u64);
    }
    output.extend_from_slice(data);
}

/// Emit a match token
fn emit_match(output: &mut Vec<u8>, offset: u16, len: usize) {
    // offset is u16 LE
    output.extend_from_slice(&offset.to_le_bytes());

    let match_code = len - 4; // match_len = match_code + 4
    if match_code <= 254 {
        output.push(match_code as u8);
    } else {
        output.push(0xFF);
        append_varint(output, (match_code - 255) as u64);
    }
}

fn append_varint(output: &mut Vec<u8>, mut value: u64) {
    loop {
        let mut byte = (value & 0x7f) as u8;
        value >>= 7;
        if value != 0 {
            byte |= 0x80;
        }
        output.push(byte);
        if value == 0 {
            break;
        }
    }
}

/// Decompress KolArk-LZ data
pub fn lz_decompress(compressed: &[u8], original_size: usize) -> Result<Vec<u8>, LzError> {
    let mut output = Vec::new();
    output
        .try_reserve_exact(original_size)
        .map_err(|_| LzError::AllocationFailed)?;
    let mut pos = 0;

    while pos < compressed.len() {
        let literal_len_field = compressed[pos];
        pos += 1;

        let literal_len = read_extended_length(compressed, &mut pos, literal_len_field)?;
        let literal_end = pos
            .checked_add(literal_len)
            .ok_or(LzError::LengthOverflow)?;
        if literal_end > compressed.len() {
            return Err(LzError::UnexpectedEof);
        }
        output.extend_from_slice(&compressed[pos..literal_end]);
        pos = literal_end;

        if output.len() > original_size {
            return Err(LzError::OutputExceedsLimit(output.len(), original_size));
        }
        if pos == compressed.len() {
            break;
        }
        if output.len() == original_size {
            return Err(LzError::TrailingBytes);
        }

        let offset_end = pos.checked_add(2).ok_or(LzError::LengthOverflow)?;
        if offset_end > compressed.len() {
            return Err(LzError::UnexpectedEof);
        }
        let offset = u16::from_le_bytes([compressed[pos], compressed[pos + 1]]);
        pos = offset_end;

        if offset == 0 {
            return Err(LzError::InvalidOffsetZero);
        }
        if offset as usize > output.len() {
            return Err(LzError::OffsetExceedsOutput(offset, output.len()));
        }

        if pos >= compressed.len() {
            return Err(LzError::UnexpectedEof);
        }
        let match_len_field = compressed[pos];
        pos += 1;
        let match_code = read_extended_length(compressed, &mut pos, match_len_field)?;
        let match_len = match_code.checked_add(4).ok_or(LzError::LengthOverflow)?;
        let output_end = output
            .len()
            .checked_add(match_len)
            .ok_or(LzError::LengthOverflow)?;
        if output_end > original_size {
            return Err(LzError::OutputExceedsLimit(output_end, original_size));
        }

        for _ in 0..match_len {
            let byte = output[output.len() - offset as usize];
            output.push(byte);
        }
    }

    if output.len() != original_size {
        return Err(LzError::OutputExceedsLimit(output.len(), original_size));
    }

    Ok(output)
}

fn read_extended_length(compressed: &[u8], pos: &mut usize, first: u8) -> Result<usize, LzError> {
    let (length, consumed) = read_extended_length_from_slice(&compressed[*pos..], first, 255)?;
    *pos = (*pos)
        .checked_add(consumed)
        .ok_or(LzError::LengthOverflow)?;
    Ok(length)
}

fn read_extended_length_from_slice(
    data: &[u8],
    first: u8,
    base: usize,
) -> Result<(usize, usize), LzError> {
    if first != 0xFF {
        return Ok((first as usize, 0));
    }

    let (extension, consumed) = read_varint_from_slice(data)?;
    let extension = usize::try_from(extension).map_err(|_| LzError::LengthOverflow)?;
    let length = base.checked_add(extension).ok_or(LzError::LengthOverflow)?;
    Ok((length, consumed))
}

/// Streaming LZ decompressor
pub struct LzDecompressor {
    original_size: usize,
    output: Vec<u8>,
    pos: usize,
    compressed: Vec<u8>,
    state: DecompressState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DecompressState {
    LiteralLen,
    LiteralData {
        remaining: usize,
    },
    MatchOffset,
    MatchLen {
        offset: u16,
    },
    MatchData {
        offset: u16,
        len: usize,
        copied: usize,
    },
    Done,
}

impl LzDecompressor {
    /// Create a new streaming decompressor
    pub fn new(original_size: usize) -> Self {
        Self {
            original_size,
            output: Vec::with_capacity(original_size),
            pos: 0,
            compressed: Vec::new(),
            state: DecompressState::LiteralLen,
        }
    }

    /// Feed compressed data
    pub fn feed(&mut self, data: &[u8]) -> Result<Vec<u8>, LzError> {
        self.compressed.extend_from_slice(data);
        let mut output_chunk = Vec::new();

        loop {
            match self.state {
                DecompressState::LiteralLen => {
                    if self.pos >= self.compressed.len() {
                        break;
                    }
                    let field = self.compressed[self.pos];
                    let (len, consumed) = match read_extended_length_from_slice(
                        &self.compressed[self.pos + 1..],
                        field,
                        255,
                    ) {
                        Ok(decoded) => decoded,
                        Err(LzError::Varint(VarintError::Truncated)) => break,
                        Err(error) => return Err(error),
                    };
                    self.pos += 1 + consumed;

                    if len > 0 {
                        self.state = DecompressState::LiteralData { remaining: len };
                    } else {
                        self.state = DecompressState::MatchOffset;
                    }
                }

                DecompressState::LiteralData { remaining } => {
                    let available = self.compressed.len() - self.pos;
                    let to_copy = remaining.min(available);
                    output_chunk.extend_from_slice(&self.compressed[self.pos..self.pos + to_copy]);
                    self.output
                        .extend_from_slice(&self.compressed[self.pos..self.pos + to_copy]);
                    self.pos += to_copy;

                    if self.output.len() > self.original_size {
                        return Err(LzError::OutputExceedsLimit(
                            self.output.len(),
                            self.original_size,
                        ));
                    }

                    if to_copy == remaining {
                        self.state = DecompressState::MatchOffset;
                    } else {
                        self.state = DecompressState::LiteralData {
                            remaining: remaining - to_copy,
                        };
                        if to_copy == 0 {
                            break;
                        }
                    }
                }

                DecompressState::MatchOffset => {
                    if self.pos + 2 > self.compressed.len() {
                        break;
                    }
                    let offset = u16::from_le_bytes([
                        self.compressed[self.pos],
                        self.compressed[self.pos + 1],
                    ]);
                    self.pos += 2;

                    if offset == 0 {
                        return Err(LzError::InvalidOffsetZero);
                    }
                    if offset as usize > self.output.len() {
                        return Err(LzError::OffsetExceedsOutput(offset, self.output.len()));
                    }

                    self.state = DecompressState::MatchLen { offset };
                }

                DecompressState::MatchLen { offset } => {
                    if self.pos >= self.compressed.len() {
                        break;
                    }
                    let field = self.compressed[self.pos];
                    let (match_code, consumed) = match read_extended_length_from_slice(
                        &self.compressed[self.pos + 1..],
                        field,
                        255,
                    ) {
                        Ok(decoded) => decoded,
                        Err(LzError::Varint(VarintError::Truncated)) => break,
                        Err(error) => return Err(error),
                    };
                    self.pos += 1 + consumed;

                    let match_len = match_code.checked_add(4).ok_or(LzError::LengthOverflow)?;
                    if match_len < 4 {
                        return Err(LzError::MatchTooSmall(match_len));
                    }
                    if self
                        .output
                        .len()
                        .checked_add(match_len)
                        .is_none_or(|end| end > self.original_size)
                    {
                        return Err(LzError::OutputExceedsLimit(
                            self.output.len().saturating_add(match_len),
                            self.original_size,
                        ));
                    }

                    self.state = DecompressState::MatchData {
                        offset,
                        len: match_len,
                        copied: 0,
                    };
                }

                DecompressState::MatchData {
                    offset,
                    len,
                    mut copied,
                } => {
                    let remaining = len - copied;
                    for _ in 0..remaining {
                        let byte = self.output[self.output.len() - offset as usize];
                        self.output.push(byte);
                        output_chunk.push(byte);
                    }
                    copied = len;

                    if copied == len {
                        if self.output.len() == self.original_size {
                            self.state = DecompressState::Done;
                        } else {
                            self.state = DecompressState::LiteralLen;
                        }
                    } else {
                        self.state = DecompressState::MatchData {
                            offset,
                            len,
                            copied,
                        };
                    }
                }

                DecompressState::Done => {
                    if self.pos != self.compressed.len() {
                        return Err(LzError::TrailingBytes);
                    }
                    break;
                }
            }
        }

        Ok(output_chunk)
    }

    /// Finalize decompression
    pub fn finish(mut self) -> Result<Vec<u8>, LzError> {
        if self.state != DecompressState::Done {
            let _ = self.feed(&[])?;
        }
        if self.original_size == 0 && self.compressed.is_empty() && self.output.is_empty() {
            return Ok(self.output);
        }
        if self.output.len() != self.original_size {
            return Err(LzError::OutputExceedsLimit(
                self.output.len(),
                self.original_size,
            ));
        }
        if self.pos != self.compressed.len() {
            return Err(LzError::UnexpectedEof);
        }
        if !matches!(
            self.state,
            DecompressState::Done | DecompressState::MatchOffset
        ) {
            return Err(LzError::UnexpectedEof);
        }
        Ok(self.output)
    }

    /// Get current output length
    pub fn output_len(&self) -> usize {
        self.output.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_lz_empty() {
        let compressed = lz_compress(b"");
        assert_eq!(compressed, Vec::<u8>::new());
        let decompressed = lz_decompress(&compressed, 0).unwrap();
        assert_eq!(decompressed, Vec::<u8>::new());
    }

    #[test]
    fn test_lz_no_matches() {
        let data = b"Hello, World!";
        let compressed = lz_compress(data);
        let decompressed = lz_decompress(&compressed, data.len()).unwrap();
        assert_eq!(decompressed, data);
    }

    #[test]
    fn test_lz_repeated() {
        let data = b"AAAAAAAAAA";
        let compressed = lz_compress(data);
        let decompressed = lz_decompress(&compressed, data.len()).unwrap();
        assert_eq!(decompressed, data);
    }

    #[test]
    fn test_lz_overlap() {
        // Overlapping match: "ABABAB" -> first "AB", then match of "ABAB" at offset 2
        let data = b"ABABAB";
        let compressed = lz_compress(data);
        let decompressed = lz_decompress(&compressed, data.len()).unwrap();
        assert_eq!(decompressed, data);
    }

    #[test]
    fn test_lz_long_literal() {
        // Literal > 254 bytes
        let data = vec![0x41u8; 300];
        let compressed = lz_compress(&data);
        let decompressed = lz_decompress(&compressed, data.len()).unwrap();
        assert_eq!(decompressed, data);
    }

    #[test]
    fn test_lz_long_match() {
        // Match > 258 bytes (254 + 4)
        let data = vec![0x41u8; 300];
        let compressed = lz_compress(&data);
        let decompressed = lz_decompress(&compressed, data.len()).unwrap();
        assert_eq!(decompressed, data);
    }

    #[test]
    fn test_lz_streaming() {
        let data = b"Hello, World! This is a test of streaming decompression.";
        let compressed = lz_compress(data);

        let mut decomp = LzDecompressor::new(data.len());
        let mut output = Vec::new();

        // Feed in small chunks
        for chunk in compressed.chunks(5) {
            let out = decomp.feed(chunk).unwrap();
            output.extend(out);
        }

        let final_out = decomp.finish().unwrap();
        assert_eq!(final_out, data);
    }

    #[test]
    fn test_lz_streaming_extended_lengths_across_every_byte_boundary() {
        let data: Vec<u8> = (0..=255).cycle().take(1024).collect();
        let compressed = lz_compress(&data);
        assert_eq!(compressed[0], 0xff);
        assert!(compressed.contains(&0xff));

        let mut decompressor = LzDecompressor::new(data.len());
        for (index, byte) in compressed.iter().enumerate() {
            decompressor
                .feed(std::slice::from_ref(byte))
                .unwrap_or_else(|error| {
                    panic!("failed at compressed byte {index} (0x{byte:02x}): {error}")
                });
        }
        assert_eq!(decompressor.finish().unwrap(), data);
    }

    #[test]
    fn test_lz_streaming_finish_rejects_partial_tokens() {
        for compressed in [
            vec![1, b'a', 0x80],
            vec![1, b'a', 1, 0],
            vec![0xff],
            vec![0xff, 1],
        ] {
            let mut decompressor = LzDecompressor::new(1);
            assert!(decompressor.feed(&compressed).is_ok());
            assert!(
                decompressor.finish().is_err(),
                "accepted incomplete stream: {compressed:?}"
            );
        }

        let mut empty = LzDecompressor::new(0);
        assert!(empty.feed(&[]).unwrap().is_empty());
        assert!(empty.finish().unwrap().is_empty());

        let mut literal_only_empty = LzDecompressor::new(0);
        literal_only_empty.feed(&[0]).unwrap();
        assert!(literal_only_empty.finish().unwrap().is_empty());
    }

    #[test]
    fn test_lz_invalid_offset_zero() {
        // Craft invalid stream with offset 0
        let mut compressed = Vec::new();
        compressed.push(0); // literal len 0
        compressed.extend_from_slice(&[0, 0]); // offset 0
        compressed.push(0); // match len 4

        let result = lz_decompress(&compressed, 4);
        assert!(matches!(result, Err(LzError::InvalidOffsetZero)));
    }

    #[test]
    fn test_lz_offset_exceeds_output() {
        // Craft invalid stream with offset > output
        let mut compressed = Vec::new();
        compressed.push(0); // literal len 0
        compressed.extend_from_slice(&[10, 0]); // offset 10
        compressed.push(0); // match len 4

        let result = lz_decompress(&compressed, 4);
        assert!(matches!(result, Err(LzError::OffsetExceedsOutput(10, 0))));
    }

    #[test]
    fn test_lz_output_exceeds_limit() {
        let data = b"Hello";
        let compressed = lz_compress(data);
        // Try to decompress with smaller original_size
        let result = lz_decompress(&compressed, 3);
        assert!(matches!(result, Err(LzError::OutputExceedsLimit(_, 3))));
    }

    #[test]
    fn test_canonical_encoder() {
        // Test that encoder follows canonical rules:
        // 1. Longest match
        // 2. Tie-break: smallest offset

        // "abcabcabc" - at position 3, we have "abcabc" matching at offset 3
        // But there's also "abc" at offset 6? No, let's think...
        // At pos 3: "abcabc" matches "abcabc" at offset 3 (len 6)
        // This should be chosen over shorter matches
        let data = b"abcabcabc";
        let compressed = lz_compress(data);
        let decompressed = lz_decompress(&compressed, data.len()).unwrap();
        assert_eq!(decompressed, data);

        // Verify the match chosen is the longest
        // At pos 3: looking back, we can match "abcabc" (6 bytes) at offset 3
        // Or "abc" (3 bytes) at offset 6 - but min match is 4, so only offset 3 works
    }
}

//! Varint (unsigned LEB128) and Zigzag signed varint encoding/decoding

use std::io::{Read, Write};

/// Maximum varint length for u64 (10 bytes)
pub const MAX_VARINT_LEN: usize = 10;

/// Varint encoding/decoding errors
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum VarintError {
    #[error("varint too long: max {} bytes", MAX_VARINT_LEN)]
    TooLong,
    #[error("varint truncated: unexpected EOF")]
    Truncated,
    #[error("varint overflow: value exceeds u64::MAX")]
    Overflow,
    #[error("non-minimal varint encoding")]
    NonMinimal,
    #[error("invalid continuation bit on final byte")]
    InvalidContinuation,
    #[error("I/O error: {0}")]
    Io(String),
}

/// Read unsigned LEB128 varint from a reader
pub fn read_varint<R: Read>(reader: &mut R) -> Result<u64, VarintError> {
    let mut bytes = [0u8; MAX_VARINT_LEN];
    for index in 0..MAX_VARINT_LEN {
        let mut byte = [0u8; 1];
        reader.read_exact(&mut byte).map_err(|error| {
            if error.kind() == std::io::ErrorKind::UnexpectedEof {
                VarintError::Truncated
            } else {
                VarintError::Io(error.to_string())
            }
        })?;
        bytes[index] = byte[0];
        if byte[0] & 0x80 == 0 {
            return read_varint_from_slice(&bytes[..=index]).map(|(value, _)| value);
        }
    }

    Err(VarintError::Overflow)
}

pub(crate) fn read_varint_from_slice(data: &[u8]) -> Result<(u64, usize), VarintError> {
    let mut result = 0u64;

    for index in 0..MAX_VARINT_LEN {
        let byte = *data.get(index).ok_or(VarintError::Truncated)?;
        if index == MAX_VARINT_LEN - 1 && byte > 1 {
            return Err(VarintError::Overflow);
        }

        result |= ((byte & 0x7f) as u64) << (index * 7);
        if byte & 0x80 == 0 {
            if index > 0 && byte & 0x7f == 0 {
                return Err(VarintError::NonMinimal);
            }
            return Ok((result, index + 1));
        }
    }

    Err(VarintError::Overflow)
}

/// Write unsigned LEB128 varint to a writer
pub fn write_varint<W: Write>(writer: &mut W, mut value: u64) -> Result<(), VarintError> {
    if value == 0 {
        writer
            .write_all(&[0])
            .map_err(|error| VarintError::Io(error.to_string()))?;
        return Ok(());
    }

    let mut buf = [0u8; MAX_VARINT_LEN];
    let mut len = 0;

    while value > 0 && len < MAX_VARINT_LEN {
        let mut byte = (value & 0x7F) as u8;
        value >>= 7;
        if value > 0 {
            byte |= 0x80;
        }
        buf[len] = byte;
        len += 1;
    }

    if value > 0 {
        return Err(VarintError::Overflow);
    }

    writer
        .write_all(&buf[..len])
        .map_err(|error| VarintError::Io(error.to_string()))?;
    Ok(())
}

/// Read signed varint (Zigzag + LEB128) from a reader
pub fn read_svarint<R: Read>(reader: &mut R) -> Result<i64, VarintError> {
    let u = read_varint(reader)?;
    Ok(decode_zigzag(u))
}

/// Write signed varint (Zigzag + LEB128) to a writer
pub fn write_svarint<W: Write>(writer: &mut W, value: i64) -> Result<(), VarintError> {
    write_varint(writer, encode_zigzag(value))
}

/// Encode signed integer to Zigzag
pub fn encode_zigzag(n: i64) -> u64 {
    ((n as u64) << 1) ^ ((n >> 63) as u64)
}

/// Decode Zigzag to signed integer
pub fn decode_zigzag(u: u64) -> i64 {
    ((u >> 1) as i64) ^ (-((u & 1) as i64))
}

/// Calculate varint encoded length for a u64
pub fn varint_len(value: u64) -> usize {
    if value == 0 {
        return 1;
    }
    let mut len = 0;
    let mut v = value;
    while v > 0 {
        v >>= 7;
        len += 1;
    }
    len
}

/// Calculate varint encoded length for an i64 (zigzag)
pub fn svarint_len(value: i64) -> usize {
    varint_len(encode_zigzag(value))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn test_varint_basic() {
        let test_cases = vec![
            (0u64, vec![0x00]),
            (1u64, vec![0x01]),
            (127u64, vec![0x7F]),
            (128u64, vec![0x80, 0x01]),
            (300u64, vec![0xAC, 0x02]),
            (16383u64, vec![0xFF, 0x7F]),
            (16384u64, vec![0x80, 0x80, 0x01]),
            (u32::MAX as u64, vec![0xFF, 0xFF, 0xFF, 0xFF, 0x0F]),
            (
                u64::MAX,
                vec![0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x01],
            ),
        ];

        for (value, expected) in test_cases {
            let mut buf = Vec::new();
            write_varint(&mut buf, value).unwrap();
            assert_eq!(buf, expected, "write_varint({})", value);

            let mut cursor = Cursor::new(&buf);
            let decoded = read_varint(&mut cursor).unwrap();
            assert_eq!(decoded, value, "read_varint({})", value);
        }
    }

    #[test]
    fn test_varint_all_width_boundaries() {
        let mut values = vec![0, 1, u64::MAX];
        for width in 1..MAX_VARINT_LEN {
            let boundary = 1u64 << (width * 7);
            values.extend([boundary - 1, boundary]);
        }

        for value in values {
            let mut encoded = Vec::new();
            write_varint(&mut encoded, value).unwrap();
            assert_eq!(encoded.len(), varint_len(value), "length for {value}");
            assert_eq!(read_varint(&mut Cursor::new(encoded)).unwrap(), value);
        }
    }

    #[test]
    fn test_slice_and_reader_decoders_agree() {
        let values = [0, 127, 128, u32::MAX as u64, 1u64 << 63, u64::MAX];
        for value in values {
            let mut encoded = Vec::new();
            write_varint(&mut encoded, value).unwrap();
            assert_eq!(read_varint_from_slice(&encoded), Ok((value, encoded.len())));
            assert_eq!(read_varint(&mut Cursor::new(encoded)).unwrap(), value);
        }

        for malformed in [
            vec![0x80],
            vec![0x81, 0],
            vec![0x80; 9],
            vec![0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 1],
            vec![0xff; 10],
        ] {
            let slice_result = read_varint_from_slice(&malformed).map(|(value, _)| value);
            let reader_result = read_varint(&mut Cursor::new(malformed));
            assert_eq!(slice_result, reader_result);
        }
    }

    #[test]
    fn test_varint_nonminimal_rejected() {
        for buf in [
            vec![0x80, 0x00],
            vec![0x81, 0x00],
            vec![0xFF, 0x00],
            vec![0x80, 0x80, 0x00],
            vec![0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x00],
        ] {
            assert_eq!(
                read_varint(&mut Cursor::new(buf)),
                Err(VarintError::NonMinimal)
            );
        }
    }

    #[test]
    fn test_varint_truncated() {
        let buf = vec![0x80]; // continuation but no more bytes
        let mut cursor = Cursor::new(buf);
        assert_eq!(read_varint(&mut cursor), Err(VarintError::Truncated));
    }

    #[test]
    fn test_varint_overflow_on_tenth_byte() {
        for final_byte in [0x02, 0x7f, 0x80, 0x81, 0xff] {
            let mut bytes = [0x80; MAX_VARINT_LEN];
            bytes[MAX_VARINT_LEN - 1] = final_byte;
            assert_eq!(
                read_varint(&mut Cursor::new(bytes)),
                Err(VarintError::Overflow),
                "tenth byte 0x{final_byte:02x}"
            );
        }
    }

    #[test]
    fn test_varint_continuation_on_tenth_byte_is_overflow() {
        let mut cursor = Cursor::new([0x80; 10]);
        assert_eq!(read_varint(&mut cursor), Err(VarintError::Overflow));
    }

    #[test]
    fn test_varint_too_long() {
        // 11 bytes all with continuation bit
        let buf = vec![0x80; 11];
        let mut cursor = Cursor::new(buf);
        assert_eq!(read_varint(&mut cursor), Err(VarintError::Overflow));
    }

    #[test]
    fn test_varint_truncated_at_every_continuation_depth() {
        for length in 1..MAX_VARINT_LEN {
            let bytes = vec![0x80; length];
            assert_eq!(
                read_varint(&mut Cursor::new(bytes)),
                Err(VarintError::Truncated),
                "truncated after {length} bytes"
            );
        }
    }

    #[test]
    fn test_varint_deterministic_round_trips() {
        let mut state = 0x4b4f_4c41_524b_7631u64;
        for _ in 0..10_000 {
            state =
                state.wrapping_add(0x9e37_79b9_7f4a_7c15).rotate_left(17) ^ 0xbf58_476d_1ce4_e5b9;
            let mut encoded = Vec::new();
            write_varint(&mut encoded, state).unwrap();
            assert_eq!(read_varint(&mut Cursor::new(encoded)).unwrap(), state);
        }
    }

    #[test]
    fn test_zigzag() {
        let test_cases = vec![
            (0i64, 0u64),
            (1i64, 2u64),
            (-1i64, 1u64),
            (2i64, 4u64),
            (-2i64, 3u64),
            (i64::MAX, (i64::MAX as u64) << 1),
            (i64::MIN, u64::MAX),
        ];

        for (signed, unsigned) in test_cases {
            assert_eq!(encode_zigzag(signed), unsigned, "encode_zigzag({})", signed);
            assert_eq!(
                decode_zigzag(unsigned),
                signed,
                "decode_zigzag({})",
                unsigned
            );
        }
    }

    #[test]
    fn test_svarint() {
        let test_cases = vec![0i64, 1, -1, 2, -2, 100, -100, i64::MAX, i64::MIN];

        for value in test_cases {
            let mut buf = Vec::new();
            write_svarint(&mut buf, value).unwrap();

            let mut cursor = Cursor::new(&buf);
            let decoded = read_svarint(&mut cursor).unwrap();
            assert_eq!(decoded, value, "svarint({})", value);
        }
    }

    #[test]
    fn test_svarint_deterministic_round_trips() {
        let mut state = 0x4b4f_4c41_524b_7631u64;
        for _ in 0..10_000 {
            state =
                state.wrapping_add(0x9e37_79b9_7f4a_7c15).rotate_left(17) ^ 0xbf58_476d_1ce4_e5b9;
            let value = state as i64;
            let mut encoded = Vec::new();
            write_svarint(&mut encoded, value).unwrap();
            assert_eq!(read_svarint(&mut Cursor::new(encoded)).unwrap(), value);
        }
    }

    #[test]
    fn test_varint_len() {
        assert_eq!(varint_len(0), 1);
        assert_eq!(varint_len(127), 1);
        assert_eq!(varint_len(128), 2);
        assert_eq!(varint_len(16383), 2);
        assert_eq!(varint_len(16384), 3);
        assert_eq!(varint_len(u32::MAX as u64), 5);
        assert_eq!(varint_len(u64::MAX), 10);
    }
}

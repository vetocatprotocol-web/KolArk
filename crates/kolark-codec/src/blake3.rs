//! BLAKE3 hashing

use blake3;
use std::io::{Read, Write};

/// BLAKE3-256 hash size
pub const HASH_SIZE: usize = 32;

/// Compute BLAKE3 hash of data
pub fn blake3_hash(data: &[u8]) -> [u8; HASH_SIZE] {
    *blake3::hash(data).as_bytes()
}

/// Compute BLAKE3 hash from a reader (streaming)
pub fn blake3_hash_streaming<R: Read>(reader: &mut R) -> std::io::Result<[u8; HASH_SIZE]> {
    let mut hasher = blake3::Hasher::new();
    let mut buffer = [0u8; 8192];
    loop {
        let n = reader.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        hasher.update(&buffer[..n]);
    }
    Ok(*hasher.finalize().as_bytes())
}

/// BLAKE3 hasher wrapper for incremental hashing
#[derive(Clone)]
pub struct Blake3Hasher {
    hasher: blake3::Hasher,
}

impl Blake3Hasher {
    /// Create a new hasher
    pub fn new() -> Self {
        Self {
            hasher: blake3::Hasher::new(),
        }
    }

    /// Update with data
    pub fn update(&mut self, data: &[u8]) {
        self.hasher.update(data);
    }

    /// Finalize and get hash
    pub fn finalize(self) -> [u8; HASH_SIZE] {
        *self.hasher.finalize().as_bytes()
    }
}

impl Default for Blake3Hasher {
    fn default() -> Self {
        Self::new()
    }
}

impl Write for Blake3Hasher {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.hasher.update(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_blake3_known_answers() {
        // From specification §19.2
        assert_eq!(
            blake3_hash(b""),
            [
                0xaf, 0x13, 0x49, 0xb9, 0xf5, 0xf9, 0xa1, 0xa6, 0xa0, 0x40, 0x4d, 0xea, 0x36, 0xdc,
                0xc9, 0x49, 0x9b, 0xcb, 0x25, 0xc9, 0xad, 0xc1, 0x12, 0xb7, 0xcc, 0x9a, 0x93, 0xca,
                0xe4, 0x1f, 0x32, 0x62,
            ]
        );

        assert_eq!(
            blake3_hash(b"abc"),
            [
                0x64, 0x37, 0xb3, 0xac, 0x38, 0x46, 0x51, 0x33, 0xff, 0xb6, 0x3b, 0x75, 0x27, 0x3a,
                0x8d, 0xb5, 0x48, 0xc5, 0x58, 0x46, 0x5d, 0x79, 0xdb, 0x03, 0xfd, 0x35, 0x9c, 0x6c,
                0xd5, 0xbd, 0x9d, 0x85,
            ]
        );

        assert_eq!(
            blake3_hash(b"Hello, World!\n"),
            [
                0x62, 0x33, 0x83, 0x4b, 0xce, 0x78, 0x17, 0xdb, 0x1e, 0x40, 0x1b, 0xce, 0xea, 0x2b,
                0x8b, 0x45, 0xc1, 0x60, 0x01, 0x15, 0xd2, 0x5b, 0x51, 0x90, 0x6c, 0x05, 0xee, 0x32,
                0x15, 0x6b, 0x51, 0xc2,
            ]
        );
    }

    #[test]
    fn test_blake3_streaming() {
        use std::io::Cursor;

        let data = b"Hello, World!\n";
        let mut cursor = Cursor::new(data);
        let hash = blake3_hash_streaming(&mut cursor).unwrap();
        assert_eq!(hash, blake3_hash(data));
    }

    #[test]
    fn test_blake3_hasher() {
        let mut hasher = Blake3Hasher::new();
        hasher.update(b"Hello, ");
        hasher.update(b"World!\n");
        let hash = hasher.finalize();
        assert_eq!(hash, blake3_hash(b"Hello, World!\n"));
    }
}

//! Content-Defined Chunking (CDC) using Gear hash

use crate::gear::gear_update;
use crate::KolArkCodecError;

/// CDC configuration parameters
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CdcConfig {
    pub chunk_size_min: u32,
    pub chunk_size_avg: u32,
    pub chunk_size_max: u32,
}

impl CdcConfig {
    /// Create new CDC config with validation
    pub fn new(
        chunk_size_min: u32,
        chunk_size_avg: u32,
        chunk_size_max: u32,
    ) -> Result<Self, KolArkCodecError> {
        if chunk_size_min < 1 {
            return Err(KolArkCodecError::InvalidConfig(
                "chunk_size_min must be >= 1".to_string(),
            ));
        }
        if !(chunk_size_min < chunk_size_avg && chunk_size_avg < chunk_size_max) {
            return Err(KolArkCodecError::InvalidConfig(
                "must have chunk_size_min < chunk_size_avg < chunk_size_max".to_string(),
            ));
        }
        Ok(Self {
            chunk_size_min,
            chunk_size_avg,
            chunk_size_max,
        })
    }

    /// Boundary span for probability calculation
    pub fn boundary_span(&self) -> u32 {
        self.chunk_size_avg - self.chunk_size_min
    }
}

impl Default for CdcConfig {
    fn default() -> Self {
        Self {
            chunk_size_min: 16384,
            chunk_size_avg: 65536,
            chunk_size_max: 262144,
        }
    }
}

/// CDC chunk result
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CdcChunk {
    pub data: Vec<u8>,
    pub offset: u64,
}

/// Split data into chunks using Gear hash CDC
pub fn cdc_split(data: &[u8], config: CdcConfig) -> Vec<CdcChunk> {
    let mut chunks = Vec::new();
    if data.is_empty() {
        return chunks;
    }

    let mut state: u32 = 0;
    let mut position: u32 = 0;
    let mut chunk_start = 0usize;
    let boundary_span = config.boundary_span();

    for (i, &byte) in data.iter().enumerate() {
        state = gear_update(state, byte);
        position += 1;

        // Force cut at maximum size
        if position == config.chunk_size_max {
            chunks.push(CdcChunk {
                data: data[chunk_start..=i].to_vec(),
                offset: chunk_start as u64,
            });
            chunk_start = i + 1;
            state = 0;
            position = 0;
            continue;
        }

        // Check for natural boundary after minimum
        if position >= config.chunk_size_min && state.is_multiple_of(boundary_span) {
            chunks.push(CdcChunk {
                data: data[chunk_start..=i].to_vec(),
                offset: chunk_start as u64,
            });
            chunk_start = i + 1;
            state = 0;
            position = 0;
        }
    }

    // Remaining data becomes final chunk (even if smaller than minimum)
    if chunk_start < data.len() {
        chunks.push(CdcChunk {
            data: data[chunk_start..].to_vec(),
            offset: chunk_start as u64,
        });
    }

    chunks
}

/// Streaming CDC splitter for large files
pub struct StreamingCdcSplitter {
    config: CdcConfig,
    state: u32,
    position: u32,
    buffer: Vec<u8>,
    total_processed: u64,
}

impl StreamingCdcSplitter {
    /// Create a new streaming CDC splitter
    pub fn new(config: CdcConfig) -> Self {
        Self {
            config,
            state: 0,
            position: 0,
            buffer: Vec::with_capacity(config.chunk_size_max as usize),
            total_processed: 0,
        }
    }

    /// Process a chunk of input data, returning completed chunks
    pub fn process(&mut self, data: &[u8]) -> Vec<CdcChunk> {
        let mut chunks = Vec::new();
        let mut i = 0;
        let boundary_span = self.config.boundary_span();

        while i < data.len() {
            let byte = data[i];
            self.buffer.push(byte);
            self.state = gear_update(self.state, byte);
            self.position += 1;
            self.total_processed += 1;
            i += 1;

            // Force cut at maximum size
            if self.position == self.config.chunk_size_max {
                chunks.push(CdcChunk {
                    data: std::mem::take(&mut self.buffer),
                    offset: self.total_processed - self.position as u64,
                });
                self.buffer = Vec::with_capacity(self.config.chunk_size_max as usize);
                self.state = 0;
                self.position = 0;
                continue;
            }

            // Check for natural boundary after minimum
            if self.position >= self.config.chunk_size_min
                && self.state.is_multiple_of(boundary_span)
            {
                chunks.push(CdcChunk {
                    data: std::mem::take(&mut self.buffer),
                    offset: self.total_processed - self.position as u64,
                });
                self.buffer = Vec::with_capacity(self.config.chunk_size_max as usize);
                self.state = 0;
                self.position = 0;
            }
        }

        chunks
    }

    /// Finalize and return any remaining data as a chunk
    pub fn finalize(&mut self) -> Option<CdcChunk> {
        if !self.buffer.is_empty() {
            Some(CdcChunk {
                data: std::mem::take(&mut self.buffer),
                offset: self.total_processed - self.position as u64,
            })
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cdc_empty() {
        let config = CdcConfig::default();
        let chunks = cdc_split(b"", config);
        assert_eq!(chunks.len(), 0);
    }

    #[test]
    fn test_cdc_small() {
        let config = CdcConfig::new(20, 30, 40).unwrap();
        let data = b"Hello, World!"; // 13 bytes, less than min
        let chunks = cdc_split(data, config);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].data, data);
    }

    #[test]
    fn test_cdc_exact_min() {
        let config = CdcConfig::new(10, 20, 30).unwrap();
        let data = vec![0x41u8; 10]; // Exactly min size
        let chunks = cdc_split(&data, config);
        assert_eq!(chunks.len(), 1);
    }

    #[test]
    fn test_cdc_force_max() {
        let config = CdcConfig::new(10, 20, 30).unwrap();
        let data = vec![0x41u8; 35]; // Larger than max
        let chunks = cdc_split(&data, config);
        // Should have at least 2 chunks (one forced at max, one remainder)
        assert!(chunks.len() >= 2);
        // First chunk should be exactly max size
        assert_eq!(chunks[0].data.len(), 30);
    }

    #[test]
    fn test_cdc_streaming() {
        let config = CdcConfig::new(20, 30, 40).unwrap();
        let mut splitter = StreamingCdcSplitter::new(config);

        let data1 = b"Hello, ";
        let chunks1 = splitter.process(data1);
        assert_eq!(chunks1.len(), 0); // Not enough data yet

        let data2 = b"World!";
        let chunks2 = splitter.process(data2);
        assert_eq!(chunks2.len(), 0); // Still not enough

        let final_chunk = splitter.finalize();
        assert!(final_chunk.is_some());
        assert_eq!(final_chunk.unwrap().data, b"Hello, World!");
    }

    #[test]
    fn test_cdc_streaming_max() {
        let config = CdcConfig::new(10, 20, 30).unwrap();
        let mut splitter = StreamingCdcSplitter::new(config);
        let data = vec![0x41u8; 40];
        let mut streamed = Vec::new();

        for input in data.chunks(10) {
            streamed.extend(splitter.process(input));
        }
        streamed.extend(splitter.finalize());

        assert_eq!(streamed, cdc_split(&data, config));
    }
}

//! Gear hash table generation using SplitMix64

/// Gear table size (256 entries)
pub const GEAR_TABLE_SIZE: usize = 256;

/// SplitMix64 constants from specification Appendix A
const SEED: u64 = 0x4B6F6C41726B0001;
const GAMMA: u64 = 0x9E3779B97F4A7C15;
const MIX1: u64 = 0xBF58476D1CE4E5B9;
const MIX2: u64 = 0x94D049BB133111EB;

const fn generate_table() -> [u32; GEAR_TABLE_SIZE] {
    let mut table = [0u32; GEAR_TABLE_SIZE];
    let mut state = SEED;

    let mut i = 0;
    while i < GEAR_TABLE_SIZE {
        state = state.wrapping_add(GAMMA);
        let mut z = (state ^ (state >> 30)).wrapping_mul(MIX1);
        z = (z ^ (z >> 27)).wrapping_mul(MIX2);
        z ^= z >> 31;
        table[i] = z as u32;
        i += 1;
    }

    table
}

/// Generate the Gear table (256 u32 values) using the canonical SplitMix64 algorithm.
pub fn generate_gear_table() -> [u32; GEAR_TABLE_SIZE] {
    generate_table()
}

/// Precomputed Gear table (256 u32 values)
/// Generated using the canonical SplitMix64 algorithm from Appendix A
pub const GEAR_TABLE_256: [u32; GEAR_TABLE_SIZE] = generate_table();

/// Get the canonical Gear table.
pub fn gear_table() -> &'static [u32; 256] {
    &GEAR_TABLE_256
}

/// Compute Gear hash state update for a single byte
#[inline]
pub fn gear_update(state: u32, byte: u8) -> u32 {
    let table = gear_table();
    state.wrapping_shl(1).wrapping_add(table[byte as usize])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_gear_table_size() {
        let table = generate_gear_table();
        assert_eq!(table.len(), 256);
    }

    #[test]
    fn test_gear_table_deterministic() {
        let table1 = generate_gear_table();
        let table2 = generate_gear_table();
        assert_eq!(table1, table2);
    }

    #[test]
    fn test_gear_table_values() {
        let table = generate_gear_table();
        assert_eq!(table, GEAR_TABLE_256);
        assert_eq!(&table[..3], &[0xf064_6379, 0x31c7_aac8, 0xb418_f38c]);
        let unique: std::collections::HashSet<_> = table.iter().collect();
        assert!(unique.len() > 200, "Gear table should have high entropy");
    }

    #[test]
    fn test_gear_update() {
        let state: u32 = 0;
        let new_state = gear_update(state, 0x41);
        assert_eq!(new_state, GEAR_TABLE_256[0x41]);
    }
}

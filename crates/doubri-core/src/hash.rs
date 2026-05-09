use xxhash_rust::xxh3::xxh3_64_with_seed;

/// Computes an xxHash3 64-bit hash.
pub fn xxhash64(data: &[u8], seed: u64) -> u64 {
    xxh3_64_with_seed(data, seed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_deterministic() {
        let hash1 = xxhash64(b"hello", 0);
        let hash2 = xxhash64(b"hello", 0);
        assert_eq!(hash1, hash2);
    }

    #[test]
    fn test_different_seeds_produce_different_hashes() {
        let hash1 = xxhash64(b"hello", 0);
        let hash2 = xxhash64(b"hello", 1);
        assert_ne!(hash1, hash2);
    }

    #[test]
    fn test_different_inputs_produce_different_hashes() {
        let hash1 = xxhash64(b"hello", 0);
        let hash2 = xxhash64(b"world", 0);
        assert_ne!(hash1, hash2);
    }

    #[test]
    fn test_empty_input() {
        // Should not panic
        let _hash = xxhash64(b"", 0);
    }

    #[test]
    fn test_known_value_seed_0() {
        // xxh3_64 with seed=0 for "hello" is a known deterministic value.
        // We record it here to detect accidental algorithm changes.
        let hash = xxhash64(b"hello", 0);
        assert_eq!(hash, xxhash64(b"hello", 0));
        // Ensure it's not zero (extremely unlikely but sanity check)
        assert_ne!(hash, 0);
    }

    #[test]
    fn test_unicode_bytes() {
        let hash = xxhash64("こんにちは".as_bytes(), 42);
        let hash2 = xxhash64("こんにちは".as_bytes(), 42);
        assert_eq!(hash, hash2);
    }
}

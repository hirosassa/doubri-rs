use std::collections::HashSet;
use std::io::{BufRead, Write};

use xxhash_rust::xxh3::Xxh3;

use crate::error::DoubriError;
use crate::format::{HashFileHeader, write_hash_data, write_hash_header};
use crate::hash::xxhash64;
use crate::ngram::char_ngrams;

/// Configuration for MinHash computation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MinHashConfig {
    /// N-gram size (default: 5).
    pub ngram_size: usize,
    /// Number of buckets (default: 40).
    pub num_buckets: usize,
    /// Number of hash values per bucket (default: 20).
    pub band_size: usize,
}

impl Default for MinHashConfig {
    fn default() -> Self {
        Self {
            ngram_size: 5,
            num_buckets: 40,
            band_size: 20,
        }
    }
}

impl MinHashConfig {
    /// Total number of hash functions (num_buckets * band_size).
    pub const fn total_hashes(&self) -> usize {
        self.num_buckets * self.band_size
    }
}

/// Computes a MinHash signature from text.
///
/// Returns a `Vec<u64>` of length `num_buckets * band_size`.
/// Layout is bucket-major: [bucket0_hash0, bucket0_hash1, ..., bucket0_hashB, bucket1_hash0, ...]
///
/// Each hash function h_i is defined as `xxhash64(ngram_bytes, seed=i)`.
/// For each hash function, the minimum value across all n-grams is kept.
///
/// If no n-grams exist (text too short), all values are `u64::MAX`.
pub fn compute_minhash(text: &str, config: &MinHashConfig) -> Vec<u64> {
    let total = config.total_hashes();
    let mut signature = vec![u64::MAX; total];

    // Deduplicate n-grams to avoid redundant hash computations
    let unique_ngrams: HashSet<&str> = char_ngrams(text, config.ngram_size).collect();

    // Pre-allocate hash buffer once, reused across n-grams
    let mut hashes = vec![0u64; total];

    for ngram in &unique_ngrams {
        let ngram_bytes = ngram.as_bytes();

        // Compute all hash values for this n-gram
        for (i, h) in hashes.iter_mut().enumerate() {
            *h = xxhash64(ngram_bytes, i as u64);
        }

        // Update minimum values (separated loop enables auto-vectorization)
        for (slot, &h) in signature.iter_mut().zip(hashes.iter()) {
            *slot = (*slot).min(h);
        }
    }

    signature
}

/// Extracts a bucket value from a MinHash signature.
///
/// `signature` is a slice of length `total_hashes` for a single document.
/// The bucket value is computed by concatenating the `band_size` hash values
/// within the bucket and hashing them. Matching bucket values indicate candidate similar documents.
pub fn bucket_value(signature: &[u64], bucket_idx: usize, config: &MinHashConfig) -> u64 {
    let start = bucket_idx * config.band_size;
    let end = start + config.band_size;
    let band = &signature[start..end];

    // Stream band hash values directly into the hasher without heap allocation
    let mut hasher = Xxh3::with_seed(0);
    for &h in band {
        hasher.update(&h.to_le_bytes());
    }
    hasher.digest()
}

/// Returns the signature slice for a given document from a flat signatures array.
#[inline]
pub fn signature_slice(signatures: &[u64], doc_idx: usize, total_hashes: usize) -> &[u64] {
    let start = doc_idx * total_hashes;
    &signatures[start..start + total_hashes]
}

/// Reads a JSONL stream, computes MinHash signatures, and writes them to a hash file.
///
/// Each line must be a JSON object with a text field specified by `text_field`.
pub fn process_jsonl<R: BufRead, W: Write>(
    reader: R,
    writer: &mut W,
    config: &MinHashConfig,
    text_field: &str,
) -> Result<u64, DoubriError> {
    let total_hashes = config.total_hashes();
    let mut signatures: Vec<u64> = Vec::new();
    let mut num_documents = 0u64;

    for line in reader.lines() {
        let line = line?;
        if line.is_empty() {
            continue;
        }
        let json: serde_json::Value = serde_json::from_str(&line)?;
        let text = json
            .get(text_field)
            .and_then(|v| v.as_str())
            .ok_or_else(|| DoubriError::MissingField {
                field: text_field.to_string(),
            })?;
        let sig = compute_minhash(text, config);
        signatures.extend_from_slice(&sig);
        num_documents += 1;
    }

    let header = HashFileHeader {
        num_documents,
        ngram_size: config.ngram_size as u32,
        num_buckets: config.num_buckets as u32,
        band_size: config.band_size as u32,
    };

    write_hash_header(writer, &header)?;
    write_hash_data(writer, &signatures, num_documents as usize, total_hashes)?;

    Ok(num_documents)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_identical_texts_produce_identical_signatures() {
        let config = MinHashConfig {
            ngram_size: 3,
            num_buckets: 5,
            band_size: 4,
        };
        let sig1 = compute_minhash("hello world foo bar", &config);
        let sig2 = compute_minhash("hello world foo bar", &config);
        assert_eq!(sig1, sig2);
    }

    #[test]
    fn test_different_texts_produce_different_signatures() {
        let config = MinHashConfig {
            ngram_size: 3,
            num_buckets: 5,
            band_size: 4,
        };
        let sig1 = compute_minhash("hello world foo bar", &config);
        let sig2 = compute_minhash("completely different text here", &config);
        assert_ne!(sig1, sig2);
    }

    #[test]
    fn test_signature_length() {
        let config = MinHashConfig {
            ngram_size: 5,
            num_buckets: 10,
            band_size: 3,
        };
        let sig = compute_minhash("this is a test sentence for minhash", &config);
        assert_eq!(sig.len(), 30); // 10 * 3
    }

    #[test]
    fn test_short_text_returns_max_values() {
        let config = MinHashConfig {
            ngram_size: 5,
            num_buckets: 3,
            band_size: 2,
        };
        // "ab" has no 5-grams
        let sig = compute_minhash("ab", &config);
        assert!(sig.iter().all(|&v| v == u64::MAX));
    }

    #[test]
    fn test_similar_texts_share_bucket_values() {
        let config = MinHashConfig {
            ngram_size: 3,
            num_buckets: 20,
            band_size: 2,
        };
        // Nearly identical texts
        let sig1 = compute_minhash("the quick brown fox jumps over the lazy dog", &config);
        let sig2 = compute_minhash("the quick brown fox jumps over the lazy cat", &config);

        // Should share at least one bucket value
        let shared = (0..config.num_buckets)
            .filter(|&b| bucket_value(&sig1, b, &config) == bucket_value(&sig2, b, &config))
            .count();
        assert!(shared > 0, "Similar texts should share at least one bucket");
    }

    #[test]
    fn test_bucket_value_deterministic() {
        let config = MinHashConfig {
            ngram_size: 3,
            num_buckets: 5,
            band_size: 4,
        };
        let sig = compute_minhash("hello world", &config);
        let bv1 = bucket_value(&sig, 0, &config);
        let bv2 = bucket_value(&sig, 0, &config);
        assert_eq!(bv1, bv2);
    }

    #[test]
    fn test_different_buckets_different_values() {
        let config = MinHashConfig {
            ngram_size: 3,
            num_buckets: 5,
            band_size: 4,
        };
        let sig = compute_minhash("hello world this is a longer text", &config);
        let bv0 = bucket_value(&sig, 0, &config);
        let bv1 = bucket_value(&sig, 1, &config);
        // Different buckets should generally produce different values
        // (not guaranteed but extremely likely)
        assert_ne!(bv0, bv1);
    }

    #[test]
    fn test_unicode_minhash() {
        let config = MinHashConfig {
            ngram_size: 3,
            num_buckets: 5,
            band_size: 4,
        };
        let sig = compute_minhash("日本語のテキストで動作確認", &config);
        assert_eq!(sig.len(), 20);
        // Should have actual hash values, not all MAX
        assert!(sig.iter().any(|&v| v != u64::MAX));
    }

    #[test]
    fn test_default_config() {
        let config = MinHashConfig::default();
        assert_eq!(config.ngram_size, 5);
        assert_eq!(config.num_buckets, 40);
        assert_eq!(config.band_size, 20);
        assert_eq!(config.total_hashes(), 800);
    }

    #[test]
    fn test_process_jsonl_basic() {
        use crate::format::{read_hash_data, read_hash_header};
        use std::io::Cursor;

        let config = MinHashConfig {
            ngram_size: 3,
            num_buckets: 2,
            band_size: 2,
        };

        let jsonl = r#"{"text":"hello world"}
{"text":"foo bar baz"}
"#;

        let mut output = Vec::new();
        let count = process_jsonl(
            std::io::BufReader::new(jsonl.as_bytes()),
            &mut output,
            &config,
            "text",
        )
        .unwrap();

        assert_eq!(count, 2);

        // Read back and verify
        let mut cursor = Cursor::new(&output);
        let header = read_hash_header(&mut cursor).unwrap();
        assert_eq!(header.num_documents, 2);
        assert_eq!(header.ngram_size, 3);
        assert_eq!(header.num_buckets, 2);
        assert_eq!(header.band_size, 2);

        let sigs = read_hash_data(&mut cursor, &header).unwrap();
        let total_hashes = 4; // 2 * 2
        assert_eq!(sigs.len(), 2 * total_hashes);

        // Verify signatures match direct computation
        let expected_sig1 = compute_minhash("hello world", &config);
        let expected_sig2 = compute_minhash("foo bar baz", &config);
        assert_eq!(&sigs[0..total_hashes], &expected_sig1[..]);
        assert_eq!(&sigs[total_hashes..2 * total_hashes], &expected_sig2[..]);
    }

    #[test]
    fn test_process_jsonl_custom_field() {
        let config = MinHashConfig {
            ngram_size: 3,
            num_buckets: 2,
            band_size: 2,
        };

        let jsonl = r#"{"content":"hello world"}"#;
        let mut output = Vec::new();
        let count = process_jsonl(
            std::io::BufReader::new(jsonl.as_bytes()),
            &mut output,
            &config,
            "content",
        )
        .unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn test_process_jsonl_missing_field() {
        let config = MinHashConfig {
            ngram_size: 3,
            num_buckets: 2,
            band_size: 2,
        };

        let jsonl = r#"{"other":"hello world"}"#;
        let mut output = Vec::new();
        let result = process_jsonl(
            std::io::BufReader::new(jsonl.as_bytes()),
            &mut output,
            &config,
            "text",
        );
        assert!(result.is_err());
    }

    #[test]
    fn test_process_jsonl_empty_lines_skipped() {
        let config = MinHashConfig {
            ngram_size: 3,
            num_buckets: 2,
            band_size: 2,
        };

        let jsonl = r#"{"text":"hello world"}

{"text":"foo bar baz"}
"#;

        let mut output = Vec::new();
        let count = process_jsonl(
            std::io::BufReader::new(jsonl.as_bytes()),
            &mut output,
            &config,
            "text",
        )
        .unwrap();
        assert_eq!(count, 2);
    }
}

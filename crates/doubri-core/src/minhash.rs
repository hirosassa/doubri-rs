use std::collections::HashSet;
use std::io::{BufRead, Write};

use rayon::prelude::*;
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

/// Computes the bucket value of one document from a band stored row-major.
///
/// `rows` holds `band_size` rows of `num_docs` values each (hash-index-major),
/// so the band of document `doc_idx` is `rows[r * num_docs + doc_idx]` for each row `r`.
/// The result equals `bucket_value` on the same band.
#[inline]
pub fn bucket_value_from_rows(
    rows: &[u64],
    num_docs: usize,
    band_size: usize,
    doc_idx: usize,
) -> u64 {
    let mut hasher = Xxh3::with_seed(0);
    for r in 0..band_size {
        hasher.update(&rows[r * num_docs + doc_idx].to_le_bytes());
    }
    hasher.digest()
}

/// Returns the signature slice for a given document from a flat signatures array.
#[inline]
pub fn signature_slice(signatures: &[u64], doc_idx: usize, total_hashes: usize) -> &[u64] {
    let start = doc_idx * total_hashes;
    &signatures[start..start + total_hashes]
}

/// Number of JSONL lines read before their signatures are computed in parallel.
const JSONL_CHUNK_SIZE: usize = 1024;

/// Reads a JSONL stream, computes MinHash signatures, and writes them to a hash file.
///
/// Each line must be a JSON object with a text field specified by `text_field`.
/// Signatures are computed in parallel; the output order matches the input order.
pub fn process_jsonl<R: BufRead, W: Write>(
    reader: R,
    writer: &mut W,
    config: &MinHashConfig,
    text_field: &str,
) -> Result<u64, DoubriError> {
    process_jsonl_chunked(reader, writer, config, text_field, JSONL_CHUNK_SIZE)
}

/// Like [`process_jsonl`], but with an explicit number of non-empty lines per parallel chunk.
fn process_jsonl_chunked<R: BufRead, W: Write>(
    reader: R,
    writer: &mut W,
    config: &MinHashConfig,
    text_field: &str,
    chunk_size: usize,
) -> Result<u64, DoubriError> {
    let chunk_size = chunk_size.max(1);
    let total_hashes = config.total_hashes();
    let mut signatures: Vec<u64> = Vec::new();
    let mut num_documents = 0u64;

    let mut lines = reader.lines();
    let mut chunk: Vec<String> = Vec::with_capacity(chunk_size);
    loop {
        chunk.clear();
        // A read error is deferred until the lines read before it have been processed,
        // so that an earlier malformed line is still reported first.
        let mut read_error = None;
        while chunk.len() < chunk_size {
            match lines.next() {
                Some(Ok(line)) if line.is_empty() => {}
                Some(Ok(line)) => chunk.push(line),
                Some(Err(e)) => {
                    read_error = Some(e);
                    break;
                }
                None => break,
            }
        }
        if chunk.is_empty() && read_error.is_none() {
            break;
        }

        // Collect every result and scan in order: rayon's `collect::<Result<..>>` does not
        // guarantee that the earliest error in input order is the one returned.
        let results: Vec<Result<Vec<u64>, DoubriError>> = chunk
            .par_iter()
            .map(|line| signature_for_line(line, config, text_field))
            .collect();
        for result in results {
            signatures.extend_from_slice(&result?);
            num_documents += 1;
        }

        if let Some(e) = read_error {
            return Err(e.into());
        }
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

fn signature_for_line(
    line: &str,
    config: &MinHashConfig,
    text_field: &str,
) -> Result<Vec<u64>, DoubriError> {
    let json: serde_json::Value = serde_json::from_str(line)?;
    let text = json
        .get(text_field)
        .and_then(|v| v.as_str())
        .ok_or_else(|| DoubriError::MissingField {
            field: text_field.to_string(),
        })?;
    Ok(compute_minhash(text, config))
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
        let err = process_jsonl(
            std::io::BufReader::new(jsonl.as_bytes()),
            &mut output,
            &config,
            "text",
        )
        .unwrap_err();
        assert!(
            matches!(err, DoubriError::MissingField { ref field } if field == "text"),
            "expected MissingField for absent text field, got {:?}",
            err
        );
    }

    #[test]
    fn test_process_jsonl_non_string_field() {
        let config = MinHashConfig {
            ngram_size: 3,
            num_buckets: 2,
            band_size: 2,
        };

        // The field exists but is not a string.
        let jsonl = r#"{"text":123}"#;
        let mut output = Vec::new();
        let err = process_jsonl(
            std::io::BufReader::new(jsonl.as_bytes()),
            &mut output,
            &config,
            "text",
        )
        .unwrap_err();
        assert!(
            matches!(err, DoubriError::MissingField { ref field } if field == "text"),
            "expected MissingField for non-string text field, got {:?}",
            err
        );
    }

    #[test]
    fn test_process_jsonl_invalid_json() {
        let config = MinHashConfig {
            ngram_size: 3,
            num_buckets: 2,
            band_size: 2,
        };

        let jsonl = "{not valid json";
        let mut output = Vec::new();
        let err = process_jsonl(
            std::io::BufReader::new(jsonl.as_bytes()),
            &mut output,
            &config,
            "text",
        )
        .unwrap_err();
        assert!(
            matches!(err, DoubriError::Json(_)),
            "expected Json error for malformed line, got {:?}",
            err
        );
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

    #[test]
    fn test_chunked_processing_preserves_document_order_across_chunk_boundaries() {
        use crate::format::{read_hash_data, read_hash_header};
        use std::io::Cursor;

        let config = MinHashConfig {
            ngram_size: 3,
            num_buckets: 2,
            band_size: 2,
        };
        let total_hashes = config.total_hashes();
        let texts = [
            "alpha beta",
            "gamma delta",
            "epsilon zeta",
            "eta theta",
            "iota kappa",
            "lambda mu",
            "nu xi omicron",
        ];
        // Empty lines are interleaved so they also fall on chunk boundaries.
        let mut jsonl = String::new();
        for (i, t) in texts.iter().enumerate() {
            jsonl.push_str(&format!("{{\"text\":\"{t}\"}}\n"));
            if i % 2 == 1 {
                jsonl.push('\n');
            }
        }

        // 1: every document is its own chunk; 3: boundary mid-input; 100: single chunk.
        for chunk_size in [1, 3, 7, 100] {
            let mut output = Vec::new();
            let count = process_jsonl_chunked(
                std::io::BufReader::new(jsonl.as_bytes()),
                &mut output,
                &config,
                "text",
                chunk_size,
            )
            .unwrap();
            assert_eq!(count, texts.len() as u64, "chunk_size {chunk_size}");

            let mut cursor = Cursor::new(&output);
            let header = read_hash_header(&mut cursor).unwrap();
            let sigs = read_hash_data(&mut cursor, &header).unwrap();
            for (i, t) in texts.iter().enumerate() {
                assert_eq!(
                    &sigs[i * total_hashes..(i + 1) * total_hashes],
                    &compute_minhash(t, &config)[..],
                    "document {i} with chunk_size {chunk_size}"
                );
            }
        }
    }

    #[test]
    fn test_chunked_processing_reports_the_earliest_error_within_a_chunk() {
        let config = MinHashConfig {
            ngram_size: 3,
            num_buckets: 2,
            band_size: 2,
        };
        // Line 2 is malformed JSON, line 5 lacks the text field. Both share one chunk,
        // so the earliest failure (line 2) must be the one reported.
        let jsonl = "{\"text\":\"ok one\"}\n{broken\n{\"text\":\"ok two\"}\n{\"text\":\"ok three\"}\n{\"other\":\"x\"}\n";

        for chunk_size in [1, 4, 100] {
            let mut output = Vec::new();
            let err = process_jsonl_chunked(
                std::io::BufReader::new(jsonl.as_bytes()),
                &mut output,
                &config,
                "text",
                chunk_size,
            )
            .unwrap_err();
            assert!(
                matches!(err, DoubriError::Json(_)),
                "chunk_size {chunk_size}: expected Json error from line 2, got {err:?}"
            );
        }
    }

    #[test]
    fn test_chunked_processing_reports_read_errors_after_valid_lines() {
        let config = MinHashConfig {
            ngram_size: 3,
            num_buckets: 2,
            band_size: 2,
        };
        // Invalid UTF-8 makes `lines()` fail with a read error after two valid lines.
        let mut input = b"{\"text\":\"ok one\"}\n{\"text\":\"ok two\"}\n".to_vec();
        input.extend_from_slice(b"\xff\xfe\n");

        for chunk_size in [1, 2, 100] {
            let mut output = Vec::new();
            let err = process_jsonl_chunked(
                std::io::BufReader::new(input.as_slice()),
                &mut output,
                &config,
                "text",
                chunk_size,
            )
            .unwrap_err();
            assert!(
                matches!(err, DoubriError::Io(_)),
                "chunk_size {chunk_size}: expected Io error, got {err:?}"
            );
        }
    }

    #[test]
    fn test_chunked_processing_reports_an_earlier_parse_error_before_a_later_read_error() {
        let config = MinHashConfig {
            ngram_size: 3,
            num_buckets: 2,
            band_size: 2,
        };
        // Line 2 is malformed JSON and line 3 is a read error. Both land in one chunk
        // for large chunk sizes; the earlier parse error must still be the one reported.
        let mut input = b"{\"text\":\"ok one\"}\n{broken\n".to_vec();
        input.extend_from_slice(b"\xff\xfe\n");

        for chunk_size in [1, 2, 100] {
            let mut output = Vec::new();
            let err = process_jsonl_chunked(
                std::io::BufReader::new(input.as_slice()),
                &mut output,
                &config,
                "text",
                chunk_size,
            )
            .unwrap_err();
            assert!(
                matches!(err, DoubriError::Json(_)),
                "chunk_size {chunk_size}: expected Json error from line 2, got {err:?}"
            );
        }
    }

    #[test]
    fn test_bucket_value_hashes_the_requested_band_with_xxh3() {
        let config = MinHashConfig {
            ngram_size: 3,
            num_buckets: 3,
            band_size: 2,
        };
        let signature = [11u64, 22, 33, 44, 55, 66];

        // Bucket values are persisted in index files, so the exact definition matters:
        // xxh3 (seed 0) over the little-endian bytes of the band's hash values.
        for (bucket_idx, band) in signature.chunks(config.band_size).enumerate() {
            let bytes: Vec<u8> = band.iter().flat_map(|h| h.to_le_bytes()).collect();
            assert_eq!(
                bucket_value(&signature, bucket_idx, &config),
                xxhash_rust::xxh3::xxh3_64_with_seed(&bytes, 0),
                "bucket {bucket_idx}"
            );
        }
    }

    #[test]
    fn test_bucket_value_from_rows_matches_bucket_value() {
        let config = MinHashConfig {
            ngram_size: 3,
            num_buckets: 3,
            band_size: 4,
        };
        let total_hashes = config.total_hashes();
        let num_docs = 5;
        let signatures: Vec<u64> = (0..num_docs * total_hashes)
            .map(|k| (k as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15))
            .collect();

        for bucket_idx in 0..config.num_buckets {
            // Rows are hash-index-major: row r holds hash (bucket_idx * band_size + r) of every doc.
            let mut rows = Vec::new();
            for r in 0..config.band_size {
                for doc in 0..num_docs {
                    rows.push(signatures[doc * total_hashes + bucket_idx * config.band_size + r]);
                }
            }
            for doc in 0..num_docs {
                let expected = bucket_value(
                    signature_slice(&signatures, doc, total_hashes),
                    bucket_idx,
                    &config,
                );
                assert_eq!(
                    bucket_value_from_rows(&rows, num_docs, config.band_size, doc),
                    expected
                );
            }
        }
    }
}

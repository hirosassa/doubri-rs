use std::fs::File;
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};

use rayon::prelude::*;

use crate::error::{DoubriError, conv};
use crate::format::{
    FLAG_DUPLICATE, FLAG_UNIQUE, HashFileHeader, IndexEntry, NUM_INDEX_SPLITS, SourceEntry,
    idx_file_path, read_hash_header, read_hash_rows, write_dup_flags, write_idx_entry,
    write_idx_header, write_src_file,
};
use crate::minhash::{MinHashConfig, bucket_value_from_rows};

/// Result of deduplication.
#[derive(Debug)]
pub struct DedupResult {
    pub total_documents: u64,
    pub duplicate_count: u64,
}

/// Document location within a group.
#[derive(Debug, Clone)]
struct DocLocation {
    /// Index of the source file.
    source_idx: u32,
    /// Document index within the source file.
    doc_idx: u32,
}

/// Pair of bucket value and document position, used for sorting.
#[derive(Debug, Clone)]
struct BucketEntry {
    bucket_value: u64,
    global_idx: u64,
}

/// Performs within-group deduplication from a list of hash files.
///
/// - `hash_file_paths`: list of `MinHash` hash file paths.
/// - `output_basename`: base name for output files (.dup, .idx.*, .src).
/// - `reverse`: if true, keeps the newer document (later file/line takes priority).
///
/// # Errors
///
/// Returns an error on I/O failure, malformed or mutually inconsistent hash files, an empty
/// file list, or when a count does not fit the integer type used for it.
pub fn dedup_group(
    hash_file_paths: &[PathBuf],
    output_basename: &str,
    reverse: bool,
) -> Result<DedupResult, DoubriError> {
    // Open every hash file and read only its header. Signatures are streamed
    // one band at a time below instead of being loaded all at once.
    let mut readers: Vec<(BufReader<File>, HashFileHeader)> = Vec::new();
    let mut source_entries: Vec<SourceEntry> = Vec::new();
    let mut doc_locations: Vec<DocLocation> = Vec::new();
    let mut config: Option<MinHashConfig> = None;
    let mut total_documents: usize = 0;

    for (source_idx, path) in hash_file_paths.iter().enumerate() {
        let mut file = BufReader::new(File::open(path)?);
        let header = read_hash_header(&mut file)?;

        // Get config from first file, verify consistency for subsequent files
        let file_config = MinHashConfig {
            ngram_size: conv(header.ngram_size, "ngram size")?,
            num_buckets: conv(header.num_buckets, "bucket count")?,
            band_size: conv(header.band_size, "band size")?,
        };
        if let Some(ref existing) = config {
            if *existing != file_config {
                return Err(DoubriError::Config {
                    msg: format!("hash file config mismatch: {existing:?} vs {file_config:?}"),
                });
            }
        } else {
            config = Some(file_config);
        }

        let source_idx: u32 = conv(source_idx, "source index")?;
        for doc_idx in 0..header.num_documents {
            doc_locations.push(DocLocation {
                source_idx,
                doc_idx: conv(doc_idx, "document index")?,
            });
        }
        let num_docs: usize = conv(header.num_documents, "document count")?;
        total_documents = total_documents
            .checked_add(num_docs)
            .ok_or(DoubriError::Overflow {
                context: "total document count",
            })?;

        source_entries.push(SourceEntry {
            item_count: header.num_documents,
            file_path: path.to_string_lossy().to_string(),
        });
        readers.push((file, header));
    }

    let config = config.ok_or_else(|| DoubriError::Config {
        msg: "no hash files provided".to_string(),
    })?;

    let mut dup_flags = vec![FLAG_UNIQUE; total_documents];

    // Bucket values are needed both for duplicate detection and for the index
    // files, so compute them once per bucket and keep them. This costs
    // 8 bytes per document per bucket, far less than the signatures.
    // Hash files are hash-index-major, so each bucket's band is the next
    // `band_size` rows of every file and the files are read sequentially once.
    let mut bucket_values: Vec<Vec<u64>> = Vec::with_capacity(config.num_buckets);
    for _ in 0..config.num_buckets {
        let mut values: Vec<u64> = Vec::with_capacity(total_documents);
        for (file, header) in &mut readers {
            let num_docs: usize = conv(header.num_documents, "document count")?;
            let rows = read_hash_rows(file, header, config.band_size)?;
            let chunk = (0..num_docs)
                .into_par_iter()
                .map(|doc| bucket_value_from_rows(&rows, num_docs, config.band_size, doc))
                .collect::<Result<Vec<u64>, DoubriError>>()?;
            values.extend(chunk);
        }
        bucket_values.push(values);
    }
    drop(readers);

    // Detect duplicates per bucket
    for values in &bucket_values {
        let mut entries: Vec<BucketEntry> = values
            .iter()
            .enumerate()
            .map(|(i, &bucket_value)| {
                Ok(BucketEntry {
                    bucket_value,
                    global_idx: conv(i, "document index")?,
                })
            })
            .collect::<Result<_, DoubriError>>()?;

        // Sort by bucket value
        entries.par_sort_unstable_by(|a, b| {
            a.bucket_value
                .cmp(&b.bucket_value)
                .then(a.global_idx.cmp(&b.global_idx))
        });

        mark_duplicates(&entries, reverse, &mut dup_flags)?;
    }

    let total_documents: u64 = conv(total_documents, "total document count")?;
    // Bounded by `dup_flags.len()`, so the sum cannot overflow `usize`.
    let duplicate_count: u64 = conv(
        dup_flags
            .iter()
            .map(|&f| usize::from(f == FLAG_DUPLICATE))
            .sum::<usize>(),
        "duplicate count",
    )?;

    // Write .dup file
    {
        let mut file = BufWriter::new(File::create(format!("{output_basename}.dup"))?);
        write_dup_flags(&mut file, &dup_flags)?;
    }

    // Write .src file
    {
        let mut file = BufWriter::new(File::create(format!("{output_basename}.src"))?);
        write_src_file(&mut file, &source_entries)?;
    }

    // Write .idx.* files (256-way split)
    // Distribute non-duplicate index entries across 256 files per bucket
    write_index_files(&bucket_values, &doc_locations, &dup_flags, output_basename)?;

    Ok(DedupResult {
        total_documents,
        duplicate_count,
    })
}

/// Marks duplicates within runs of entries (sorted by bucket value) sharing a bucket value.
fn mark_duplicates(
    entries: &[BucketEntry],
    reverse: bool,
    dup_flags: &mut [u8],
) -> Result<(), DoubriError> {
    for group in entries.chunk_by(|a, b| a.bucket_value == b.bucket_value) {
        // Keep the newer one (mark all but the last) when `reverse`,
        // otherwise keep the older one (mark all but the first).
        let duplicates = if reverse {
            group.split_last().map(|(_, rest)| rest)
        } else {
            group.split_first().map(|(_, rest)| rest)
        }
        .ok_or(DoubriError::OutOfRange {
            context: "empty bucket group",
        })?;
        for entry in duplicates {
            let idx: usize = conv(entry.global_idx, "document index")?;
            *dup_flags.get_mut(idx).ok_or(DoubriError::OutOfRange {
                context: "duplicate flag index",
            })? = FLAG_DUPLICATE;
        }
    }
    Ok(())
}

// The split index is the low byte of the bucket value (`bv & 0xFF`).
const _: () = assert!(NUM_INDEX_SPLITS == 256);

/// Writes index files with 256-way split.
///
/// `bucket_values[bucket_idx][doc]` holds the precomputed bucket value of each
/// document. Entries are appended per split in bucket order, then document order.
fn write_index_files(
    bucket_values: &[Vec<u64>],
    doc_locations: &[DocLocation],
    dup_flags: &[u8],
    basename: &str,
) -> Result<(), DoubriError> {
    // Writers are opened lazily and kept open for the whole run.
    let mut writers: Vec<Option<BufWriter<File>>> = (0..NUM_INDEX_SPLITS).map(|_| None).collect();

    for values in bucket_values {
        // `zip` would silently truncate on a length mismatch, so reject it up front.
        if values.len() != dup_flags.len() || values.len() != doc_locations.len() {
            return Err(DoubriError::OutOfRange {
                context: "index input lengths",
            });
        }
        for ((&bv, &flag), loc) in values.iter().zip(dup_flags).zip(doc_locations) {
            if flag == FLAG_DUPLICATE {
                continue;
            }
            let split_byte: u8 = conv(bv & 0xFF, "index split")?;
            let slot = writers
                .get_mut(usize::from(split_byte))
                .ok_or(DoubriError::OutOfRange {
                    context: "index split",
                })?;
            if slot.is_none() {
                let path = idx_file_path(basename, split_byte);
                let file_exists = Path::new(&path).exists();
                let mut writer = BufWriter::new(
                    std::fs::OpenOptions::new()
                        .create(true)
                        .append(true)
                        .open(&path)?,
                );
                if !file_exists {
                    write_idx_header(&mut writer)?;
                }
                *slot = Some(writer);
            }
            let writer = slot.as_mut().ok_or(DoubriError::OutOfRange {
                context: "index writer",
            })?;
            write_idx_entry(
                writer,
                &IndexEntry {
                    group: loc.source_idx,
                    item_number: loc.doc_idx,
                    bucket_value: bv,
                },
            )?;
        }
    }

    for writer in writers.iter_mut().flatten() {
        writer.flush()?;
    }

    Ok(())
}

/// Reads a list of hash file paths from stdin.
///
/// # Errors
///
/// Returns an error if reading a line fails.
pub fn read_hash_file_list<R: BufRead>(reader: R) -> Result<Vec<PathBuf>, DoubriError> {
    let mut paths = Vec::new();
    for line in reader.lines() {
        let line = line?;
        let trimmed = line.trim();
        if !trimmed.is_empty() {
            paths.push(PathBuf::from(trimmed));
        }
    }
    Ok(paths)
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    clippy::as_conversions,
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss,
    clippy::string_slice,
    clippy::format_push_string,
    clippy::format_collect,
    clippy::naive_bytecount,
    reason = "test code"
)]
mod tests {
    use super::*;
    use crate::format::{read_dup_flags, read_hash_data, read_src_file};
    use crate::minhash::process_jsonl;
    use crate::minhash::{bucket_value, signature_slice};
    use tempfile::TempDir;

    fn create_hash_file(dir: &Path, name: &str, texts: &[&str]) -> PathBuf {
        let config = MinHashConfig {
            ngram_size: 3,
            num_buckets: 3,
            band_size: 2,
        };

        let mut jsonl = String::new();
        for text in texts {
            jsonl.push_str(&format!("{{\"text\":\"{text}\"}}\n"));
        }

        let path = dir.join(name);
        let mut file = File::create(&path).unwrap();
        process_jsonl(BufReader::new(jsonl.as_bytes()), &mut file, &config, "text").unwrap();
        path
    }

    #[test]
    fn test_dedup_no_duplicates() {
        let dir = TempDir::new().unwrap();
        let hash_path = create_hash_file(
            dir.path(),
            "test.hash",
            &[
                "hello world foo",
                "completely different text here",
                "another unique document",
            ],
        );

        let basename = dir.path().join("output").to_string_lossy().to_string();
        let result = dedup_group(&[hash_path], &basename, false).unwrap();

        assert_eq!(result.total_documents, 3);
        // These texts are different enough that no duplicates should be found
        // (though with small params, collisions are possible)

        // Verify .dup file exists
        let dup_path = format!("{basename}.dup");
        let flags = read_dup_flags(&mut File::open(&dup_path).unwrap()).unwrap();
        assert_eq!(flags.len(), 3);
    }

    #[test]
    fn test_dedup_with_identical_documents() {
        let dir = TempDir::new().unwrap();
        let hash_path = create_hash_file(
            dir.path(),
            "test.hash",
            &[
                "this is a test document with enough text",
                "this is a test document with enough text",
                "completely different text for testing",
            ],
        );

        let basename = dir.path().join("output").to_string_lossy().to_string();
        let result = dedup_group(&[hash_path], &basename, false).unwrap();

        assert_eq!(result.total_documents, 3);
        assert!(
            result.duplicate_count >= 1,
            "identical docs should be detected as duplicates"
        );

        // Verify: first occurrence should be unique, second should be duplicate
        let dup_path = format!("{basename}.dup");
        let flags = read_dup_flags(&mut File::open(&dup_path).unwrap()).unwrap();
        assert_eq!(flags[0], FLAG_UNIQUE);
        assert_eq!(flags[1], FLAG_DUPLICATE);
    }

    #[test]
    fn test_dedup_reverse_keeps_newer() {
        let dir = TempDir::new().unwrap();
        let hash_path = create_hash_file(
            dir.path(),
            "test.hash",
            &[
                "this is a test document with enough text",
                "this is a test document with enough text",
            ],
        );

        let basename = dir.path().join("output").to_string_lossy().to_string();
        let result = dedup_group(&[hash_path], &basename, true).unwrap();

        assert_eq!(result.total_documents, 2);
        assert_eq!(result.duplicate_count, 1);

        let dup_path = format!("{basename}.dup");
        let flags = read_dup_flags(&mut File::open(&dup_path).unwrap()).unwrap();
        // reverse: first one is marked as duplicate, second is kept
        assert_eq!(flags[0], FLAG_DUPLICATE);
        assert_eq!(flags[1], FLAG_UNIQUE);
    }

    #[test]
    fn test_dedup_multiple_hash_files() {
        let dir = TempDir::new().unwrap();
        let hash_path1 = create_hash_file(
            dir.path(),
            "file1.hash",
            &["document one with enough text to hash"],
        );
        let hash_path2 = create_hash_file(
            dir.path(),
            "file2.hash",
            &["document one with enough text to hash"],
        );

        let basename = dir.path().join("output").to_string_lossy().to_string();
        let result = dedup_group(&[hash_path1, hash_path2], &basename, false).unwrap();

        assert_eq!(result.total_documents, 2);
        assert_eq!(result.duplicate_count, 1);

        // Verify .src file
        let src_path = format!("{basename}.src");
        let entries = read_src_file(&mut File::open(&src_path).unwrap()).unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].item_count, 1);
        assert_eq!(entries[1].item_count, 1);
    }

    #[test]
    fn test_dedup_writes_src_file() {
        let dir = TempDir::new().unwrap();
        let hash_path = create_hash_file(dir.path(), "test.hash", &["hello world foo bar baz"]);

        let basename = dir.path().join("output").to_string_lossy().to_string();
        dedup_group(std::slice::from_ref(&hash_path), &basename, false).unwrap();

        let src_path = format!("{basename}.src");
        let entries = read_src_file(&mut File::open(&src_path).unwrap()).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].item_count, 1);
        assert!(entries[0].file_path.contains("test.hash"));
    }

    #[test]
    fn test_read_hash_file_list() {
        let input = "file1.hash\nfile2.hash\n\nfile3.hash\n";
        let paths = read_hash_file_list(BufReader::new(input.as_bytes())).unwrap();
        assert_eq!(
            paths,
            vec![
                PathBuf::from("file1.hash"),
                PathBuf::from("file2.hash"),
                PathBuf::from("file3.hash"),
            ]
        );
    }

    #[test]
    fn test_dedup_empty_input() {
        let err = dedup_group(&[], "output", false).unwrap_err();
        assert!(
            matches!(err, DoubriError::Config { .. }),
            "expected Config error for no hash files, got {err:?}"
        );
    }

    /// Writes a hash file with an explicit config so callers can create
    /// files with mismatching parameters.
    fn create_hash_file_with_config(
        dir: &Path,
        name: &str,
        texts: &[&str],
        config: &MinHashConfig,
    ) -> PathBuf {
        let mut jsonl = String::new();
        for text in texts {
            jsonl.push_str(&format!("{{\"text\":\"{text}\"}}\n"));
        }
        let path = dir.join(name);
        let mut file = File::create(&path).unwrap();
        process_jsonl(BufReader::new(jsonl.as_bytes()), &mut file, config, "text").unwrap();
        path
    }

    #[test]
    fn test_dedup_config_mismatch() {
        let dir = TempDir::new().unwrap();
        let config_a = MinHashConfig {
            ngram_size: 3,
            num_buckets: 3,
            band_size: 2,
        };
        let config_b = MinHashConfig {
            ngram_size: 3,
            num_buckets: 3,
            band_size: 4, // differs from config_a
        };
        let path_a =
            create_hash_file_with_config(dir.path(), "a.hash", &["hello world foo"], &config_a);
        let path_b =
            create_hash_file_with_config(dir.path(), "b.hash", &["hello world foo"], &config_b);

        let basename = dir.path().join("output").to_string_lossy().to_string();
        let err = dedup_group(&[path_a, path_b], &basename, false).unwrap_err();
        assert!(
            matches!(err, DoubriError::Config { .. }),
            "expected Config error for mismatched configs, got {err:?}"
        );
    }

    #[test]
    fn test_dedup_three_identical_forward_marks_all_but_first() {
        let dir = TempDir::new().unwrap();
        let text = "this is a test document with enough text to hash";
        let hash_path = create_hash_file(dir.path(), "test.hash", &[text, text, text]);

        let basename = dir.path().join("output").to_string_lossy().to_string();
        let result = dedup_group(&[hash_path], &basename, false).unwrap();

        assert_eq!(result.total_documents, 3);
        assert_eq!(result.duplicate_count, 2);

        let flags = read_dup_flags(&mut File::open(format!("{basename}.dup")).unwrap()).unwrap();
        assert_eq!(flags, vec![FLAG_UNIQUE, FLAG_DUPLICATE, FLAG_DUPLICATE]);
    }

    #[test]
    fn test_dedup_three_identical_reverse_marks_all_but_last() {
        let dir = TempDir::new().unwrap();
        let text = "this is a test document with enough text to hash";
        let hash_path = create_hash_file(dir.path(), "test.hash", &[text, text, text]);

        let basename = dir.path().join("output").to_string_lossy().to_string();
        let result = dedup_group(&[hash_path], &basename, true).unwrap();

        assert_eq!(result.total_documents, 3);
        assert_eq!(result.duplicate_count, 2);

        let flags = read_dup_flags(&mut File::open(format!("{basename}.dup")).unwrap()).unwrap();
        assert_eq!(flags, vec![FLAG_DUPLICATE, FLAG_DUPLICATE, FLAG_UNIQUE]);
    }

    #[test]
    fn test_dedup_index_files_content() {
        use crate::format::{NUM_INDEX_SPLITS, idx_file_path, read_idx_entry, read_idx_header};

        let dir = TempDir::new().unwrap();
        // Two distinct texts; count uniques from the .dup file rather than assuming.
        let hash_path = create_hash_file(
            dir.path(),
            "test.hash",
            &[
                "hello world foo bar baz",
                "completely different content here",
            ],
        );
        let num_buckets = 3; // matches create_hash_file config

        let basename = dir.path().join("output").to_string_lossy().to_string();
        dedup_group(&[hash_path], &basename, false).unwrap();

        let flags = read_dup_flags(&mut File::open(format!("{basename}.dup")).unwrap()).unwrap();
        let unique_count = flags.iter().filter(|&&f| f == FLAG_UNIQUE).count();

        let mut total_entries = 0usize;
        for split_idx in 0..NUM_INDEX_SPLITS {
            let path = idx_file_path(&basename, split_idx as u8);
            if !Path::new(&path).exists() {
                continue;
            }
            let mut reader = BufReader::new(File::open(&path).unwrap());
            // Header must be present and valid.
            read_idx_header(&mut reader).unwrap();
            while let Some(entry) = read_idx_entry(&mut reader).unwrap() {
                // Each entry must land in the split matching its bucket value.
                assert_eq!(
                    (entry.bucket_value as usize) % NUM_INDEX_SPLITS,
                    split_idx,
                    "index entry placed in wrong split"
                );
                // Entry references must be within range of the single source.
                assert_eq!(entry.group, 0);
                assert!((entry.item_number as usize) < flags.len());
                total_entries += 1;
            }
        }

        // Every unique document contributes exactly one entry per bucket.
        assert_eq!(total_entries, unique_count * num_buckets);
    }

    #[test]
    fn test_dedup_output_matches_independently_computed_expectation() {
        use crate::format::{read_idx_entry, read_idx_header};
        use std::collections::HashSet;

        let dir = TempDir::new().unwrap();
        let config = MinHashConfig {
            ngram_size: 3,
            num_buckets: 3,
            band_size: 2,
        };
        let texts_a = [
            "the quick brown fox jumps over the lazy dog",
            "completely different content for the second doc",
            "the quick brown fox jumps over the lazy dog",
            "yet another unique sentence about rust programming",
        ];
        let texts_b = [
            "completely different content for the second doc",
            "a fresh document that only appears in file b",
            "yet another unique sentence about rust programming",
        ];
        let path_a = create_hash_file_with_config(dir.path(), "a.hash", &texts_a, &config);
        let path_b = create_hash_file_with_config(dir.path(), "b.hash", &texts_b, &config);

        // Independently compute every document's bucket values from the hash files.
        let mut docs: Vec<(u32, u32, Vec<u64>)> = Vec::new();
        for (source_idx, path) in [&path_a, &path_b].into_iter().enumerate() {
            let mut file = BufReader::new(File::open(path).unwrap());
            let header = read_hash_header(&mut file).unwrap();
            let sigs = read_hash_data(&mut file, &header).unwrap();
            for doc_idx in 0..header.num_documents as usize {
                let sig = signature_slice(&sigs, doc_idx, config.total_hashes().unwrap()).unwrap();
                let bvs = (0..config.num_buckets)
                    .map(|b| bucket_value(sig, b, &config).unwrap())
                    .collect();
                docs.push((source_idx as u32, doc_idx as u32, bvs));
            }
        }

        // Forward mode: a document is a duplicate if an earlier one shares any bucket value.
        let mut seen: Vec<HashSet<u64>> = vec![HashSet::new(); config.num_buckets];
        let mut expected_dup = Vec::new();
        for (_, _, bvs) in &docs {
            let is_dup = bvs.iter().enumerate().any(|(b, v)| seen[b].contains(v));
            for (b, v) in bvs.iter().enumerate() {
                seen[b].insert(*v);
            }
            expected_dup.push(is_dup);
        }
        assert!(expected_dup.iter().any(|&d| d), "test needs duplicates");
        assert!(expected_dup.iter().any(|&d| !d), "test needs uniques");

        let basename = dir.path().join("output").to_string_lossy().to_string();
        dedup_group(&[path_a, path_b], &basename, false).unwrap();

        let flags = read_dup_flags(&mut File::open(format!("{basename}.dup")).unwrap()).unwrap();
        let expected_flags: Vec<u8> = expected_dup
            .iter()
            .map(|&d| if d { FLAG_DUPLICATE } else { FLAG_UNIQUE })
            .collect();
        assert_eq!(flags, expected_flags);

        // Index files: per split, entries ordered by bucket, then by document order.
        for split_idx in 0..NUM_INDEX_SPLITS {
            let mut expected: Vec<(u32, u32, u64)> = Vec::new();
            for b in 0..config.num_buckets {
                for (i, (group, item, bvs)) in docs.iter().enumerate() {
                    if !expected_dup[i] && (bvs[b] as usize) % NUM_INDEX_SPLITS == split_idx {
                        expected.push((*group, *item, bvs[b]));
                    }
                }
            }

            let path = idx_file_path(&basename, split_idx as u8);
            if expected.is_empty() {
                assert!(
                    !Path::new(&path).exists(),
                    "unexpected file for split {split_idx}"
                );
                continue;
            }
            let mut reader = BufReader::new(File::open(&path).unwrap());
            read_idx_header(&mut reader).unwrap();
            let mut actual = Vec::new();
            while let Some(e) = read_idx_entry(&mut reader).unwrap() {
                actual.push((e.group, e.item_number, e.bucket_value));
            }
            assert_eq!(actual, expected, "split {split_idx} differs");
        }
    }
}

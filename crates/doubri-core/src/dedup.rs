use std::fs::File;
use std::io::{BufRead, BufReader, BufWriter};
use std::path::{Path, PathBuf};

use rayon::prelude::*;

use crate::error::DoubriError;
use crate::format::{
    FLAG_DUPLICATE, FLAG_UNIQUE, IndexEntry, NUM_INDEX_SPLITS, SourceEntry, idx_file_path,
    read_hash_data, read_hash_header, write_dup_flags, write_idx_entry, write_idx_header,
    write_src_file,
};
use crate::minhash::{MinHashConfig, bucket_value, signature_slice};

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
/// - `hash_file_paths`: list of MinHash hash file paths.
/// - `output_basename`: base name for output files (.dup, .idx.*, .src).
/// - `reverse`: if true, keeps the newer document (later file/line takes priority).
pub fn dedup_group(
    hash_file_paths: &[PathBuf],
    output_basename: &str,
    reverse: bool,
) -> Result<DedupResult, DoubriError> {
    // Read all hash files into a flat signature array
    let mut all_signatures: Vec<u64> = Vec::new();
    let mut source_entries: Vec<SourceEntry> = Vec::new();
    let mut doc_locations: Vec<DocLocation> = Vec::new();
    let mut config: Option<MinHashConfig> = None;
    let mut total_documents: usize = 0;

    for (source_idx, path) in hash_file_paths.iter().enumerate() {
        let mut file = BufReader::new(File::open(path)?);
        let header = read_hash_header(&mut file)?;

        // Get config from first file, verify consistency for subsequent files
        let file_config = MinHashConfig {
            ngram_size: header.ngram_size as usize,
            num_buckets: header.num_buckets as usize,
            band_size: header.band_size as usize,
        };
        if let Some(ref existing) = config {
            if *existing != file_config {
                return Err(DoubriError::Config {
                    msg: format!(
                        "hash file config mismatch: {:?} vs {:?}",
                        existing, file_config
                    ),
                });
            }
        } else {
            config = Some(file_config);
        }

        let signatures = read_hash_data(&mut file, &header)?;
        let num_docs = header.num_documents as usize;

        for doc_idx in 0..num_docs {
            doc_locations.push(DocLocation {
                source_idx: source_idx as u32,
                doc_idx: doc_idx as u32,
            });
        }
        all_signatures.extend_from_slice(&signatures);
        total_documents += num_docs;

        source_entries.push(SourceEntry {
            item_count: header.num_documents,
            file_path: path.to_string_lossy().to_string(),
        });
    }

    let config = config.ok_or_else(|| DoubriError::Config {
        msg: "no hash files provided".to_string(),
    })?;

    let total_hashes = config.total_hashes();
    let mut dup_flags = vec![FLAG_UNIQUE; total_documents];

    // Detect duplicates per bucket
    for bucket_idx in 0..config.num_buckets {
        // Compute bucket values
        let mut entries: Vec<BucketEntry> = (0..total_documents)
            .into_par_iter()
            .map(|i| {
                let sig = signature_slice(&all_signatures, i, total_hashes);
                BucketEntry {
                    bucket_value: bucket_value(sig, bucket_idx, &config),
                    global_idx: i as u64,
                }
            })
            .collect();

        // Sort by bucket value
        entries.par_sort_unstable_by(|a, b| {
            a.bucket_value
                .cmp(&b.bucket_value)
                .then(a.global_idx.cmp(&b.global_idx))
        });

        // Mark duplicates within groups sharing the same bucket value
        let mut i = 0;
        while i < entries.len() {
            let mut j = i + 1;
            while j < entries.len() && entries[j].bucket_value == entries[i].bucket_value {
                j += 1;
            }

            // entries[i..j] share the same bucket value
            if j - i > 1 {
                if reverse {
                    // Keep the newer one (mark all but the last)
                    for entry in &entries[i..j - 1] {
                        dup_flags[entry.global_idx as usize] = FLAG_DUPLICATE;
                    }
                } else {
                    // Keep the older one (mark all but the first)
                    for entry in &entries[i + 1..j] {
                        dup_flags[entry.global_idx as usize] = FLAG_DUPLICATE;
                    }
                }
            }

            i = j;
        }
    }

    let total_documents = total_documents as u64;
    let duplicate_count = dup_flags.iter().filter(|&&f| f == FLAG_DUPLICATE).count() as u64;

    // Write .dup file
    {
        let mut file = BufWriter::new(File::create(format!("{}.dup", output_basename))?);
        write_dup_flags(&mut file, &dup_flags)?;
    }

    // Write .src file
    {
        let mut file = BufWriter::new(File::create(format!("{}.src", output_basename))?);
        write_src_file(&mut file, &source_entries)?;
    }

    // Write .idx.* files (256-way split)
    // Distribute non-duplicate index entries across 256 files per bucket
    write_index_files(
        &all_signatures,
        total_documents as usize,
        total_hashes,
        &doc_locations,
        &dup_flags,
        &config,
        output_basename,
    )?;

    Ok(DedupResult {
        total_documents,
        duplicate_count,
    })
}

/// Writes index files with 256-way split.
fn write_index_files(
    signatures: &[u64],
    num_docs: usize,
    total_hashes: usize,
    doc_locations: &[DocLocation],
    dup_flags: &[u8],
    config: &MinHashConfig,
    basename: &str,
) -> Result<(), DoubriError> {
    for bucket_idx in 0..config.num_buckets {
        // Build bucket_value -> split_idx -> entries mapping
        let mut split_entries: Vec<Vec<IndexEntry>> = vec![Vec::new(); NUM_INDEX_SPLITS];

        for i in 0..num_docs {
            if dup_flags[i] == FLAG_DUPLICATE {
                continue;
            }
            let sig = signature_slice(signatures, i, total_hashes);
            let bv = bucket_value(sig, bucket_idx, config);
            let split_idx = (bv as usize) % NUM_INDEX_SPLITS;
            let loc = &doc_locations[i];
            split_entries[split_idx].push(IndexEntry {
                group: loc.source_idx,
                item_number: loc.doc_idx,
                bucket_value: bv,
            });
        }

        // Write to each split file (append mode)
        for (split_idx, entries) in split_entries.iter().enumerate() {
            if entries.is_empty() {
                continue;
            }
            let path = idx_file_path(basename, split_idx as u8);
            let file_exists = Path::new(&path).exists();
            let mut file = BufWriter::new(
                std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(&path)?,
            );

            if !file_exists {
                write_idx_header(&mut file)?;
            }

            for entry in entries {
                write_idx_entry(&mut file, entry)?;
            }
        }
    }

    Ok(())
}

/// Reads a list of hash file paths from stdin.
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
mod tests {
    use super::*;
    use crate::format::{read_dup_flags, read_src_file};
    use crate::minhash::process_jsonl;
    use tempfile::TempDir;

    fn create_hash_file(dir: &Path, name: &str, texts: &[&str]) -> PathBuf {
        let config = MinHashConfig {
            ngram_size: 3,
            num_buckets: 3,
            band_size: 2,
        };

        let mut jsonl = String::new();
        for text in texts {
            jsonl.push_str(&format!("{{\"text\":\"{}\"}}\n", text));
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
        let dup_path = format!("{}.dup", basename);
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
        let dup_path = format!("{}.dup", basename);
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

        let dup_path = format!("{}.dup", basename);
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
        let src_path = format!("{}.src", basename);
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
        dedup_group(&[hash_path.clone()], &basename, false).unwrap();

        let src_path = format!("{}.src", basename);
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
        let result = dedup_group(&[], "output", false);
        assert!(result.is_err());
    }
}

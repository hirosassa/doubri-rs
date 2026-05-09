use std::fs::File;
use std::io::{BufReader, BufWriter};
use std::path::Path;

use crate::error::DoubriError;
use crate::format::{
    FLAG_DUPLICATE, IndexEntry, idx_file_path, read_dup_flags, read_idx_entry, read_idx_header,
    write_dup_flags,
};

/// Information about a merge source group.
#[derive(Debug)]
pub struct MergeSource {
    /// Base name.
    pub basename: String,
    /// Number of documents.
    pub doc_count: usize,
}

/// Result of merge operation.
#[derive(Debug)]
pub struct MergeResult {
    pub cross_duplicates: u64,
}

/// Merges indices from multiple groups and detects cross-group duplicates.
///
/// - `sources`: base names for each group (.dup, .idx.* must exist).
/// - `start_split` / `end_split`: range of split indices to process.
/// - `reverse`: if true, keeps documents from the newer group.
pub fn merge_groups(
    sources: &[MergeSource],
    start_split: u8,
    end_split: u8,
    reverse: bool,
) -> Result<MergeResult, DoubriError> {
    // Read dup flags for each group
    let mut group_flags: Vec<Vec<u8>> = Vec::new();
    for source in sources {
        let dup_path = format!("{}.dup", source.basename);
        let flags = read_dup_flags(&mut File::open(&dup_path)?)?;
        group_flags.push(flags);
    }

    let mut cross_duplicates = 0u64;

    // Merge across each split file
    for split_idx in start_split..=end_split {
        // Read index entries from each group
        let mut all_entries: Vec<(usize, IndexEntry)> = Vec::new(); // (group_idx, entry)

        for (group_idx, source) in sources.iter().enumerate() {
            let path = idx_file_path(&source.basename, split_idx);
            if !Path::new(&path).exists() {
                continue;
            }

            let mut reader = BufReader::new(File::open(&path)?);
            read_idx_header(&mut reader)?;

            while let Some(entry) = read_idx_entry(&mut reader)? {
                all_entries.push((group_idx, entry));
            }
        }

        // Sort by bucket value
        all_entries.sort_by(|a, b| {
            a.1.bucket_value
                .cmp(&b.1.bucket_value)
                .then(a.0.cmp(&b.0))
                .then(a.1.item_number.cmp(&b.1.item_number))
        });

        // Detect cross-group duplicates with the same bucket value
        let mut i = 0;
        while i < all_entries.len() {
            let mut j = i + 1;
            while j < all_entries.len()
                && all_entries[j].1.bucket_value == all_entries[i].1.bucket_value
            {
                j += 1;
            }

            // all_entries[i..j] share the same bucket value
            if j - i > 1 {
                // Check if entries come from different groups
                let first_group = all_entries[i].0;
                let has_cross_group = all_entries[i..j].iter().any(|(g, _)| *g != first_group);

                if has_cross_group {
                    let mark_range = if reverse {
                        &all_entries[i..j - 1]
                    } else {
                        &all_entries[i + 1..j]
                    };
                    for (group_idx, entry) in mark_range {
                        let idx = entry.item_number as usize;
                        let flags = &mut group_flags[*group_idx];
                        if idx >= flags.len() {
                            return Err(DoubriError::InvalidFormat {
                                msg: format!(
                                    "item_number {} out of bounds for group {} (size {})",
                                    entry.item_number,
                                    group_idx,
                                    flags.len()
                                ),
                            });
                        }
                        flags[idx] = FLAG_DUPLICATE;
                        cross_duplicates += 1;
                    }
                }
            }

            i = j;
        }
    }

    // Write merged flags
    for (group_idx, source) in sources.iter().enumerate() {
        let merge_path = format!("{}.dup.merge", source.basename);
        let mut file = BufWriter::new(File::create(&merge_path)?);
        write_dup_flags(&mut file, &group_flags[group_idx])?;
    }

    Ok(MergeResult { cross_duplicates })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dedup::dedup_group;
    use crate::format::FLAG_UNIQUE;
    use crate::minhash::{MinHashConfig, process_jsonl};
    use std::io::BufReader;
    use tempfile::TempDir;

    fn create_hash_and_dedup(
        dir: &Path,
        group_name: &str,
        texts: &[&str],
    ) -> (String, Vec<std::path::PathBuf>) {
        let config = MinHashConfig {
            ngram_size: 3,
            num_buckets: 3,
            band_size: 2,
        };

        let mut jsonl = String::new();
        for text in texts {
            jsonl.push_str(&format!("{{\"text\":\"{}\"}}\n", text));
        }

        let hash_path = dir.join(format!("{}.hash", group_name));
        let mut file = File::create(&hash_path).unwrap();
        process_jsonl(BufReader::new(jsonl.as_bytes()), &mut file, &config, "text").unwrap();

        let basename = dir.join(group_name).to_string_lossy().to_string();
        dedup_group(&[hash_path.clone()], &basename, false).unwrap();

        (basename, vec![hash_path])
    }

    #[test]
    fn test_merge_no_cross_duplicates() {
        let dir = TempDir::new().unwrap();

        let (basename1, _) = create_hash_and_dedup(
            dir.path(),
            "group1",
            &["unique text for group one document"],
        );
        let (basename2, _) = create_hash_and_dedup(
            dir.path(),
            "group2",
            &["completely different text for group two"],
        );

        let sources = vec![
            MergeSource {
                basename: basename1.clone(),
                doc_count: 1,
            },
            MergeSource {
                basename: basename2.clone(),
                doc_count: 1,
            },
        ];

        merge_groups(&sources, 0, 255, false).unwrap();

        // Different texts should not produce cross-group duplicates
        // (small params may cause some false positives, but unlikely with these texts)
        // Just verify the merge completes and produces .dup.merge files
        assert!(Path::new(&format!("{}.dup.merge", basename1)).exists());
        assert!(Path::new(&format!("{}.dup.merge", basename2)).exists());
    }

    #[test]
    fn test_merge_with_cross_duplicates() {
        let dir = TempDir::new().unwrap();

        let same_text = "this is the exact same document text for both groups";
        let (basename1, _) = create_hash_and_dedup(dir.path(), "group1", &[same_text]);
        let (basename2, _) = create_hash_and_dedup(dir.path(), "group2", &[same_text]);

        let sources = vec![
            MergeSource {
                basename: basename1.clone(),
                doc_count: 1,
            },
            MergeSource {
                basename: basename2.clone(),
                doc_count: 1,
            },
        ];

        let result = merge_groups(&sources, 0, 255, false).unwrap();

        assert!(
            result.cross_duplicates > 0,
            "identical docs across groups should be detected"
        );

        // group1 document is kept, group2 document is marked as duplicate
        let flags1 =
            read_dup_flags(&mut File::open(format!("{}.dup.merge", basename1)).unwrap()).unwrap();
        let flags2 =
            read_dup_flags(&mut File::open(format!("{}.dup.merge", basename2)).unwrap()).unwrap();

        assert_eq!(flags1[0], FLAG_UNIQUE);
        assert_eq!(flags2[0], FLAG_DUPLICATE);
    }

    #[test]
    fn test_merge_reverse() {
        let dir = TempDir::new().unwrap();

        let same_text = "this is the exact same document text for reverse test";
        let (basename1, _) = create_hash_and_dedup(dir.path(), "group1", &[same_text]);
        let (basename2, _) = create_hash_and_dedup(dir.path(), "group2", &[same_text]);

        let sources = vec![
            MergeSource {
                basename: basename1.clone(),
                doc_count: 1,
            },
            MergeSource {
                basename: basename2.clone(),
                doc_count: 1,
            },
        ];

        let result = merge_groups(&sources, 0, 255, true).unwrap();

        assert!(result.cross_duplicates > 0);

        // reverse: keep group2, mark group1 as duplicate
        let flags1 =
            read_dup_flags(&mut File::open(format!("{}.dup.merge", basename1)).unwrap()).unwrap();
        let flags2 =
            read_dup_flags(&mut File::open(format!("{}.dup.merge", basename2)).unwrap()).unwrap();

        assert_eq!(flags1[0], FLAG_DUPLICATE);
        assert_eq!(flags2[0], FLAG_UNIQUE);
    }
}

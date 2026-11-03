//! Property tests comparing `dedup_group` against a simple exact-match reference model.
//!
//! Documents are drawn from a pool of mutually dissimilar texts, so MinHash/LSH
//! must flag exactly the documents whose text already appeared (or appears later,
//! in reverse mode).

use std::collections::HashSet;
use std::fs::File;
use std::io::BufReader;
use std::path::PathBuf;

use doubri_core::dedup::dedup_group;
use doubri_core::format::{FLAG_DUPLICATE, FLAG_UNIQUE, read_dup_flags};
use doubri_core::minhash::{MinHashConfig, process_jsonl};
use proptest::prelude::*;
use tempfile::TempDir;

const POOL: [&str; 6] = [
    "The quick brown fox jumps over the lazy dog near the quiet river bank",
    "Distributed consensus protocols tolerate failures by replicating a log across nodes",
    "Mountain tea is harvested by hand early in the morning before the mist clears",
    "Quantum error correction encodes logical qubits redundantly into physical qubits",
    "A sourdough starter needs regular feeding with flour and water to stay healthy",
    "Medieval trade routes connected distant markets through caravans and sea voyages",
];

fn config() -> MinHashConfig {
    MinHashConfig {
        ngram_size: 5,
        num_buckets: 20,
        band_size: 10,
    }
}

/// Writes one hash file per chunk and returns their paths.
fn write_hash_files(dir: &TempDir, chunks: &[Vec<usize>]) -> Vec<PathBuf> {
    chunks
        .iter()
        .enumerate()
        .map(|(i, chunk)| {
            let jsonl: String = chunk
                .iter()
                .map(|&t| format!("{{\"text\":\"{}\"}}\n", POOL[t]))
                .collect();
            let path = dir.path().join(format!("{i}.hash"));
            let mut file = File::create(&path).unwrap();
            process_jsonl(
                BufReader::new(jsonl.as_bytes()),
                &mut file,
                &config(),
                "text",
            )
            .unwrap();
            path
        })
        .collect()
}

/// Reference model: a document is a duplicate iff an identical text comes earlier
/// (forward) or later (reverse) in the concatenated document order.
fn expected_flags(texts: &[usize], reverse: bool) -> Vec<u8> {
    let mut seen = HashSet::new();
    let mut flags = vec![FLAG_UNIQUE; texts.len()];
    let order: Vec<usize> = if reverse {
        (0..texts.len()).rev().collect()
    } else {
        (0..texts.len()).collect()
    };
    for i in order {
        if !seen.insert(texts[i]) {
            flags[i] = FLAG_DUPLICATE;
        }
    }
    flags
}

fn chunks_strategy() -> impl Strategy<Value = Vec<Vec<usize>>> {
    // 1-3 hash files, each holding 1-6 documents
    prop::collection::vec(prop::collection::vec(0..POOL.len(), 1..6), 1..4)
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(32))]

    #[test]
    fn dedup_flags_match_exact_match_model(chunks in chunks_strategy(), reverse in any::<bool>()) {
        let dir = TempDir::new().unwrap();
        let paths = write_hash_files(&dir, &chunks);
        let basename = dir.path().join("g").to_string_lossy().to_string();

        let result = dedup_group(&paths, &basename, reverse).unwrap();
        let flags = read_dup_flags(&mut File::open(format!("{basename}.dup")).unwrap()).unwrap();

        let texts: Vec<usize> = chunks.iter().flatten().copied().collect();
        let expected = expected_flags(&texts, reverse);

        prop_assert_eq!(&flags, &expected);
        prop_assert_eq!(result.total_documents, texts.len() as u64);
        prop_assert_eq!(
            result.duplicate_count,
            expected.iter().filter(|&&f| f == FLAG_DUPLICATE).count() as u64
        );
    }

    #[test]
    fn dedup_keeps_exactly_one_document_per_distinct_text(chunks in chunks_strategy(), reverse in any::<bool>()) {
        let dir = TempDir::new().unwrap();
        let paths = write_hash_files(&dir, &chunks);
        let basename = dir.path().join("g").to_string_lossy().to_string();

        dedup_group(&paths, &basename, reverse).unwrap();
        let flags = read_dup_flags(&mut File::open(format!("{basename}.dup")).unwrap()).unwrap();

        let texts: Vec<usize> = chunks.iter().flatten().copied().collect();
        let kept: Vec<usize> = texts
            .iter()
            .zip(&flags)
            .filter(|&(_, &f)| f == FLAG_UNIQUE)
            .map(|(&t, _)| t)
            .collect();
        let distinct: HashSet<usize> = texts.iter().copied().collect();

        prop_assert_eq!(kept.len(), distinct.len());
        prop_assert_eq!(kept.iter().copied().collect::<HashSet<_>>(), distinct);
    }
}

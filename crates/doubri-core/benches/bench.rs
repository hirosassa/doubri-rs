use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};

use std::fs::File;
use std::io::{BufWriter, Cursor};
use std::path::{Path, PathBuf};

use doubri_core::apply::apply_whole;
use doubri_core::dedup::dedup_group;
use doubri_core::format::{
    FLAG_DUPLICATE, FLAG_UNIQUE, HashFileHeader, read_hash_data, read_hash_header, write_hash_data,
    write_hash_header,
};
use doubri_core::merge::{MergeSource, merge_groups};
use doubri_core::minhash::{MinHashConfig, bucket_value, compute_minhash, process_jsonl};
use doubri_core::ngram::char_ngrams;
use doubri_core::similarity::jaccard_similarity;

// --- Data generators ---

fn generate_ascii_text(size: usize) -> String {
    let words = [
        "the", "quick", "brown", "fox", "jumps", "over", "lazy", "dog", "and", "cat", "runs",
        "fast", "through", "green", "field", "under", "blue", "sky", "with", "warm", "wind",
    ];
    let mut text = String::with_capacity(size);
    let mut i = 0;
    while text.len() < size {
        if !text.is_empty() {
            text.push(' ');
        }
        text.push_str(words[i % words.len()]);
        i += 1;
    }
    text.truncate(size);
    text
}

/// Deterministic pseudo-random generator (LCG) so benchmarks need no extra dependency.
struct Lcg(u64);

impl Lcg {
    fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        self.0 >> 33
    }
}

/// Generates ASCII text with many distinct n-grams (closer to real documents than the
/// cyclic word list above, whose unique n-gram count saturates quickly).
fn generate_random_ascii_text(size: usize, seed: u64) -> String {
    let mut rng = Lcg(seed);
    let mut text = String::with_capacity(size);
    while text.len() < size {
        let word_len = 2 + (rng.next() % 9) as usize;
        for _ in 0..word_len {
            text.push((b'a' + (rng.next() % 26) as u8) as char);
        }
        text.push(' ');
    }
    text.truncate(size);
    text
}

fn generate_random_jsonl(num_docs: usize, doc_size: usize) -> String {
    let mut jsonl = String::new();
    for i in 0..num_docs {
        let text = generate_random_ascii_text(doc_size, i as u64 + 1);
        jsonl.push_str(&format!("{{\"text\":\"{}\"}}\n", text));
    }
    jsonl
}

fn generate_cjk_text(size: usize) -> String {
    let chars: Vec<char> = "日本語のテキスト処理を行うための文字列です大規模言語モデルの学習データから重複文書を検出する".chars().collect();
    let mut text = String::with_capacity(size);
    let mut i = 0;
    while text.len() < size {
        text.push(chars[i % chars.len()]);
        i += 1;
    }
    text
}

fn generate_jsonl(num_docs: usize, doc_size: usize) -> String {
    let text = generate_ascii_text(doc_size);
    let mut jsonl = String::new();
    for i in 0..num_docs {
        // Vary text slightly per document
        let suffix = format!(" doc{}", i);
        jsonl.push_str(&format!(
            "{{\"text\":\"{}{}\"}}\n",
            &text[..doc_size.saturating_sub(suffix.len())],
            suffix
        ));
    }
    jsonl
}

fn make_similar_text(base: &str, similarity: f64) -> String {
    let chars: Vec<char> = base.chars().collect();
    let change_count = ((1.0 - similarity) * chars.len() as f64) as usize;
    let mut result: Vec<char> = chars.clone();
    for i in 0..change_count.min(result.len()) {
        result[i] = 'X';
    }
    result.into_iter().collect()
}

// --- Benchmarks ---

fn bench_char_ngrams(c: &mut Criterion) {
    let mut group = c.benchmark_group("char_ngrams");
    group.sample_size(20);

    for &size in &[1_000, 10_000, 100_000] {
        for &(label, ngram_size) in &[("n3", 3), ("n5", 5)] {
            let ascii = generate_ascii_text(size);
            group.throughput(Throughput::Bytes(size as u64));
            group.bench_with_input(
                BenchmarkId::new(format!("ascii/{}", label), size),
                &ascii,
                |b, text| b.iter(|| char_ngrams(text, ngram_size).count()),
            );

            let cjk = generate_cjk_text(size);
            group.bench_with_input(
                BenchmarkId::new(format!("cjk/{}", label), size),
                &cjk,
                |b, text| b.iter(|| char_ngrams(text, ngram_size).count()),
            );
        }
    }
    group.finish();
}

fn bench_compute_minhash(c: &mut Criterion) {
    let mut group = c.benchmark_group("compute_minhash");
    group.sample_size(10);

    let configs = [
        (
            "b20r40",
            MinHashConfig {
                ngram_size: 5,
                num_buckets: 40,
                band_size: 20,
            },
        ),
        (
            "b8r14",
            MinHashConfig {
                ngram_size: 5,
                num_buckets: 14,
                band_size: 8,
            },
        ),
    ];

    for &size in &[1_000, 10_000, 100_000] {
        for (config_label, config) in &configs {
            let ascii = generate_ascii_text(size);
            group.throughput(Throughput::Bytes(size as u64));
            group.bench_with_input(
                BenchmarkId::new(format!("ascii/{}", config_label), size),
                &ascii,
                |b, text| b.iter(|| compute_minhash(text, config)),
            );

            let cjk = generate_cjk_text(size);
            group.bench_with_input(
                BenchmarkId::new(format!("cjk/{}", config_label), size),
                &cjk,
                |b, text| b.iter(|| compute_minhash(text, config)),
            );

            let random = generate_random_ascii_text(size, 42);
            group.bench_with_input(
                BenchmarkId::new(format!("ascii_random/{}", config_label), size),
                &random,
                |b, text| b.iter(|| compute_minhash(text, config)),
            );
        }
    }
    group.finish();
}

fn bench_bucket_value(c: &mut Criterion) {
    let mut group = c.benchmark_group("bucket_value");

    let config = MinHashConfig {
        ngram_size: 5,
        num_buckets: 40,
        band_size: 20,
    };
    let text = generate_ascii_text(10_000);
    let signature = compute_minhash(&text, &config);

    group.bench_function("b20r40", |b| {
        b.iter(|| bucket_value(&signature, 0, &config))
    });
    group.finish();
}

fn bench_jaccard_similarity(c: &mut Criterion) {
    let mut group = c.benchmark_group("jaccard_similarity");
    group.sample_size(20);

    for &size in &[1_000, 10_000] {
        let text_a = generate_ascii_text(size);

        // Identical texts
        let text_b_identical = text_a.clone();
        group.bench_with_input(
            BenchmarkId::new("identical", size),
            &(&text_a, &text_b_identical),
            |b, (a, bb)| b.iter(|| jaccard_similarity(a, bb, 5)),
        );

        // Similar texts (~90%)
        let text_b_similar = make_similar_text(&text_a, 0.9);
        group.bench_with_input(
            BenchmarkId::new("similar_90pct", size),
            &(&text_a, &text_b_similar),
            |b, (a, bb)| b.iter(|| jaccard_similarity(a, bb, 5)),
        );

        // Unrelated texts
        let text_b_unrelated = generate_cjk_text(size);
        group.bench_with_input(
            BenchmarkId::new("unrelated", size),
            &(&text_a, &text_b_unrelated),
            |b, (a, bb)| b.iter(|| jaccard_similarity(a, bb, 5)),
        );
    }
    group.finish();
}

fn bench_process_jsonl(c: &mut Criterion) {
    let mut group = c.benchmark_group("process_jsonl");
    group.sample_size(10);

    let config = MinHashConfig {
        ngram_size: 5,
        num_buckets: 40,
        band_size: 20,
    };

    for &num_docs in &[100, 1000] {
        let jsonl = generate_jsonl(num_docs, 500);
        group.throughput(Throughput::Elements(num_docs as u64));
        group.bench_with_input(BenchmarkId::new("ascii", num_docs), &jsonl, |b, data| {
            b.iter(|| {
                let mut output = Vec::new();
                process_jsonl(
                    std::io::BufReader::new(data.as_bytes()),
                    &mut output,
                    &config,
                    "text",
                )
                .unwrap();
            })
        });
    }

    for &num_docs in &[100, 1000] {
        let jsonl = generate_random_jsonl(num_docs, 500);
        group.throughput(Throughput::Elements(num_docs as u64));
        group.bench_with_input(
            BenchmarkId::new("ascii_random", num_docs),
            &jsonl,
            |b, data| {
                b.iter(|| {
                    let mut output = Vec::new();
                    process_jsonl(
                        std::io::BufReader::new(data.as_bytes()),
                        &mut output,
                        &config,
                        "text",
                    )
                    .unwrap();
                })
            },
        );
    }
    group.finish();
}

// --- Pipeline stages beyond MinHash ---

const PIPELINE_CONFIG: MinHashConfig = MinHashConfig {
    ngram_size: 5,
    num_buckets: 14,
    band_size: 8,
};

/// Builds a flat signature array where every 10th document duplicates an earlier one.
fn synthetic_signatures(num_docs: usize, seed: u64) -> Vec<u64> {
    let total_hashes = PIPELINE_CONFIG.total_hashes();
    let mut rng = Lcg(seed);
    let mut signatures = vec![0u64; num_docs * total_hashes];
    for doc in 0..num_docs {
        if doc >= 10 && doc % 10 == 0 {
            let src = doc - 10;
            signatures.copy_within(
                src * total_hashes..(src + 1) * total_hashes,
                doc * total_hashes,
            );
        } else {
            for h in &mut signatures[doc * total_hashes..(doc + 1) * total_hashes] {
                *h = rng.next();
            }
        }
    }
    signatures
}

fn write_hash_file(path: &Path, signatures: &[u64], num_docs: usize) {
    let header = HashFileHeader {
        num_documents: num_docs as u64,
        ngram_size: PIPELINE_CONFIG.ngram_size as u32,
        num_buckets: PIPELINE_CONFIG.num_buckets as u32,
        band_size: PIPELINE_CONFIG.band_size as u32,
    };
    let mut writer = BufWriter::new(File::create(path).unwrap());
    write_hash_header(&mut writer, &header).unwrap();
    write_hash_data(
        &mut writer,
        signatures,
        num_docs,
        PIPELINE_CONFIG.total_hashes(),
    )
    .unwrap();
}

fn bench_hash_file_io(c: &mut Criterion) {
    let mut group = c.benchmark_group("hash_file_io");
    group.sample_size(10);

    let num_docs = 100_000;
    let total_hashes = PIPELINE_CONFIG.total_hashes();
    let signatures = synthetic_signatures(num_docs, 1);
    let header = HashFileHeader {
        num_documents: num_docs as u64,
        ngram_size: PIPELINE_CONFIG.ngram_size as u32,
        num_buckets: PIPELINE_CONFIG.num_buckets as u32,
        band_size: PIPELINE_CONFIG.band_size as u32,
    };
    group.throughput(Throughput::Elements(num_docs as u64));

    group.bench_function("write_hash_data/100k", |b| {
        b.iter(|| {
            let mut out = Vec::with_capacity(num_docs * total_hashes * 8);
            write_hash_data(&mut out, &signatures, num_docs, total_hashes).unwrap();
            out
        })
    });

    let mut encoded = Vec::new();
    write_hash_header(&mut encoded, &header).unwrap();
    write_hash_data(&mut encoded, &signatures, num_docs, total_hashes).unwrap();
    group.bench_function("read_hash_data/100k", |b| {
        b.iter(|| {
            let mut cursor = Cursor::new(&encoded);
            let header = read_hash_header(&mut cursor).unwrap();
            read_hash_data(&mut cursor, &header).unwrap()
        })
    });
    group.finish();
}

/// Writes `num_groups` hash files and returns their paths.
fn prepare_hash_files(dir: &Path, num_groups: usize, docs_per_group: usize) -> Vec<PathBuf> {
    (0..num_groups)
        .map(|g| {
            let path = dir.join(format!("group{g}.hash"));
            // Same seed across groups so that cross-group duplicates exist for merge.
            let signatures = synthetic_signatures(docs_per_group, 7 + (g as u64 % 2));
            write_hash_file(&path, &signatures, docs_per_group);
            path
        })
        .collect()
}

fn bench_dedup_group(c: &mut Criterion) {
    let mut group = c.benchmark_group("dedup_group");
    group.sample_size(10);

    let dir = tempfile::tempdir().unwrap();
    let num_docs = 100_000;
    let hash_files = prepare_hash_files(dir.path(), 1, num_docs);
    group.throughput(Throughput::Elements(num_docs as u64));

    let mut run = 0u64;
    group.bench_function("100k/b8r14", |b| {
        b.iter(|| {
            // idx files are opened in append mode, so each run needs a fresh basename.
            run += 1;
            let basename = dir.path().join(format!("out{run}"));
            dedup_group(&hash_files, basename.to_str().unwrap(), false).unwrap()
        })
    });
    group.finish();
}

fn bench_merge_groups(c: &mut Criterion) {
    let mut group = c.benchmark_group("merge_groups");
    group.sample_size(10);

    let dir = tempfile::tempdir().unwrap();
    let docs_per_group = 50_000;
    let hash_files = prepare_hash_files(dir.path(), 2, docs_per_group);
    let sources: Vec<MergeSource> = hash_files
        .iter()
        .enumerate()
        .map(|(g, hash_file)| {
            let basename = dir.path().join(format!("merge_src{g}"));
            dedup_group(
                std::slice::from_ref(hash_file),
                basename.to_str().unwrap(),
                false,
            )
            .unwrap();
            MergeSource {
                basename: basename.to_string_lossy().into_owned(),
                doc_count: docs_per_group,
            }
        })
        .collect();

    group.throughput(Throughput::Elements(2 * docs_per_group as u64));
    group.bench_function("2x50k/b8r14", |b| {
        b.iter(|| merge_groups(&sources, 0, 255, false).unwrap())
    });
    group.finish();
}

fn bench_apply_whole(c: &mut Criterion) {
    let mut group = c.benchmark_group("apply_whole");

    let num_docs = 100_000;
    let jsonl = generate_jsonl(num_docs, 500);
    let flags: Vec<u8> = (0..num_docs)
        .map(|i| {
            if i % 3 == 0 {
                FLAG_UNIQUE
            } else {
                FLAG_DUPLICATE
            }
        })
        .collect();
    group.throughput(Throughput::Bytes(jsonl.len() as u64));

    group.bench_function("100k_docs", |b| {
        b.iter(|| {
            let mut out = Vec::with_capacity(jsonl.len());
            apply_whole(jsonl.as_bytes(), &mut out, &flags).unwrap();
            out
        })
    });
    group.finish();
}

criterion_group!(
    benches,
    bench_char_ngrams,
    bench_compute_minhash,
    bench_bucket_value,
    bench_jaccard_similarity,
    bench_process_jsonl,
    bench_hash_file_io,
    bench_dedup_group,
    bench_merge_groups,
    bench_apply_whole,
);
criterion_main!(benches);

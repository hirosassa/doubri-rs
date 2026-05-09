use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};

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
    group.finish();
}

criterion_group!(
    benches,
    bench_char_ngrams,
    bench_compute_minhash,
    bench_bucket_value,
    bench_jaccard_similarity,
    bench_process_jsonl,
);
criterion_main!(benches);

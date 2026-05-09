use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};

use doubri_core::minhash::{MinHashConfig, compute_minhash};
use doubri_core::ngram::char_ngrams;
use doubri_core::similarity::jaccard_similarity;

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

fn bench_char_ngrams(c: &mut Criterion) {
    let mut group = c.benchmark_group("char_ngrams");

    for &size in &[1_000, 10_000] {
        let text = generate_ascii_text(size);
        group.throughput(Throughput::Bytes(size as u64));
        group.bench_with_input(BenchmarkId::new("ascii/n5", size), &text, |b, text| {
            b.iter(|| char_ngrams(text, 5).count())
        });
    }
    group.finish();
}

fn bench_compute_minhash(c: &mut Criterion) {
    let mut group = c.benchmark_group("compute_minhash");
    group.sample_size(10);

    let config = MinHashConfig {
        ngram_size: 5,
        num_buckets: 40,
        band_size: 20,
    };

    for &size in &[1_000, 10_000] {
        let text = generate_ascii_text(size);
        group.throughput(Throughput::Bytes(size as u64));
        group.bench_with_input(BenchmarkId::new("ascii/b20r40", size), &text, |b, text| {
            b.iter(|| compute_minhash(text, &config))
        });
    }
    group.finish();
}

fn bench_jaccard_similarity(c: &mut Criterion) {
    let mut group = c.benchmark_group("jaccard_similarity");

    let text_a = generate_ascii_text(1_000);
    let text_b = generate_ascii_text(1_000);
    group.bench_function("1kb", |b| {
        b.iter(|| jaccard_similarity(&text_a, &text_b, 5))
    });
    group.finish();
}

criterion_group!(
    benches,
    bench_char_ngrams,
    bench_compute_minhash,
    bench_jaccard_similarity,
);
criterion_main!(benches);

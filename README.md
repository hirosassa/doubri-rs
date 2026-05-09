# doubri-rs

[![build](https://github.com/hirosassa/doubri-rs/actions/workflows/test.yaml/badge.svg)](https://github.com/hirosassa/doubri-rs/actions/workflows/test.yaml)
[![codecov](https://codecov.io/gh/hirosassa/doubri-rs/branch/main/graph/badge.svg?token=Q5FIA58YTN)](https://codecov.io/gh/hirosassa/doubri-rs)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](https://github.com/hirosassa/doubri-rs/blob/main/LICENSE)

A Rust implementation of [doubri](https://github.com/swallow-llm/doubri), a large-scale document deduplication toolkit using MinHash and Locality-Sensitive Hashing (LSH).

doubri-rs detects and removes near-duplicate documents from massive text corpora (e.g., Common Crawl) for LLM training data preprocessing. It runs on a single server without requiring distributed infrastructure like Spark or HDFS.

## Features

- MinHash + LSH based near-duplicate detection at scale
- Unicode-aware character-level n-gram processing with an optimized ASCII fast path
- Parallel processing via Rayon for sorting and bucket computation
- Memory-efficient flat signature storage (single contiguous allocation)
- Streaming I/O for processing large JSONL datasets
- Multi-stage pipeline: minhash, dedup, merge, and apply

## Installation

Download a prebuilt binary from [GitHub Releases](https://github.com/hirosassa/doubri-rs/releases):

```bash
# Example for Linux x86_64
curl -LO https://github.com/hirosassa/doubri-rs/releases/latest/download/doubri-x86_64-unknown-linux-gnu.tar.gz
tar xzf doubri-x86_64-unknown-linux-gnu.tar.gz
```

Available targets: `x86_64-unknown-linux-gnu`, `aarch64-unknown-linux-gnu`, `x86_64-apple-darwin`, `aarch64-apple-darwin`

Or build from source:

```bash
cargo build --release
```

## Quick Start

```bash
# 1. Compute MinHash signatures from JSONL
cat documents.jsonl | doubri minhash output.hash

# 2. Detect duplicates within a group
echo "output.hash" | doubri dedup result

# 3. Apply deduplication (output only unique documents)
cat documents.jsonl | doubri apply-whole -f result.dup > unique.jsonl
```

## Commands

### `doubri minhash`

Computes MinHash signatures from a JSONL stream and writes them to a binary hash file.

```bash
cat input.jsonl | doubri minhash [OPTIONS] <OUTPUT>
```

| Option | Default | Description |
|--------|---------|-------------|
| `-n, --ngram` | 5 | N-gram size |
| `-b, --band-size` | 20 | Number of hash values per bucket |
| `-r, --num-buckets` | 40 | Number of buckets |
| `-t, --text-field` | `text` | JSON field name containing the text |

### `doubri dedup`

Detects duplicates within a group of hash files. Reads hash file paths from stdin (one per line).

```bash
echo "output.hash" | doubri dedup [OPTIONS] <BASENAME>
```

| Option | Description |
|--------|-------------|
| `-r, --reverse` | Keep the newer document instead of the older one |

Outputs: `<BASENAME>.dup`, `<BASENAME>.idx.*`, `<BASENAME>.src`

### `doubri merge`

Detects cross-group duplicates by merging indices from multiple dedup results.

```bash
doubri merge [OPTIONS] <SOURCE1> <SOURCE2> ...
```

| Option | Default | Description |
|--------|---------|-------------|
| `-r, --reverse` | | Keep the newer group's documents |
| `-s, --start` | 0 | Start split index |
| `-e, --end` | 255 | End split index |

Outputs: `<SOURCE>.dup.merge` for each source

### `doubri similarity`

Computes pairwise Jaccard similarity between all documents (for verification purposes).

```bash
cat input.jsonl | doubri similarity [OPTIONS]
```

| Option | Default | Description |
|--------|---------|-------------|
| `-n, --ngram` | 5 | N-gram size |
| `-s, --threshold` | 0.6 | Minimum similarity threshold |
| `-i, --id-field` | `id` | JSON field name for document ID |
| `-t, --text-field` | `text` | JSON field name for text |

### `doubri apply-whole`

Filters a JSONL stream, outputting only non-duplicate documents.

```bash
cat all_documents.jsonl | doubri apply-whole -f result.dup > unique.jsonl
```

### `doubri apply-each`

Filters a single source's JSONL stream using the dedup results.

```bash
cat source.jsonl | doubri apply-each -f result.dup -s result.src <TARGET>
```

## Full Pipeline Example

```bash
# Step 1: Compute MinHash for each source
cat source1.jsonl | doubri minhash source1.hash
cat source2.jsonl | doubri minhash source2.hash

# Step 2: Deduplicate within each group
printf "source1.hash\nsource2.hash" | doubri dedup group1

# Step 3: (Optional) Cross-group deduplication
# If you have multiple groups, merge their indices:
# doubri merge group1 group2

# Step 4: Extract unique documents
cat source1.jsonl source2.jsonl | doubri apply-whole -f group1.dup > unique.jsonl
```

## Algorithm

doubri-rs uses MinHash with Locality-Sensitive Hashing (LSH) for near-duplicate detection:

1. N-gram extraction: Text is split into character-level n-grams (default: 5-grams)
2. MinHash signatures: For each document, compute `b * r` hash values (default: 40 * 20 = 800) using xxHash with different seeds. Each value is the minimum hash across all n-grams for that seed
3. LSH bucketing: The 800 hash values are divided into `r` buckets of `b` values each. Documents sharing identical bucket values in any bucket are candidate duplicates
4. Deduplication: Candidates are sorted by bucket value and duplicates are marked

With default parameters (b=20, r=40), documents with Jaccard similarity >= 0.9 are detected with ~92.5% probability.

## Differences from the Original C++ Implementation

- CLI interface: Single binary with subcommands (`doubri minhash`) instead of separate binaries (`doubri-minhash`)
- File format: Uses its own binary format (magic header `DoubriR1`) not compatible with the C++ version
- Performance: Comparable to the C++ implementation (within ~2% on benchmarks)
- Memory: Flat contiguous signature storage instead of per-document allocations
- Safety: Bounds-checked index access when reading external files

## Benchmarks

Run benchmarks:

```bash
cargo bench --bench bench_light -p doubri-core  # Quick (~30s)
cargo bench --bench bench -p doubri-core         # Full (~5min)
```

## Acknowledgements

This project is a Rust port of [doubri](https://github.com/swallow-llm/doubri) by the [Swallow LLM](https://github.com/swallow-llm) team at Tokyo Institute of Technology.

## License

MIT License. See [LICENSE](LICENSE) for details.

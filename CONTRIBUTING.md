# Contributing to doubri-rs

## Prerequisites

- Rust (stable, latest recommended)
- [cargo-release](https://github.com/crate-ci/cargo-release) (for releases)

## Development Setup

```bash
git clone https://github.com/hirosassa/doubri-rs.git
cd doubri-rs
cargo build
cargo test --workspace
```

## Code Style

Run `cargo fmt` and `cargo clippy -- -D warnings -W clippy::nursery` and fix all warnings

## Testing

```bash
cargo test --workspace           # Run all tests
cargo test -p doubri-core        # Core library only
cargo test -p doubri-core -- ngram  # Specific module
```

## Benchmarks

```bash
cargo bench --bench bench_light -p doubri-core  # Quick (~30s)
cargo bench --bench bench -p doubri-core         # Full (~5min)
```

## CI

Pushes to `main` and pull requests run:
- `cargo check`, `cargo fmt --check`, `cargo clippy`, `cargo test` on stable/beta/nightly
- Code coverage via cargo-llvm-cov, uploaded to Codecov

## Making a Release

Releases are managed with [cargo-release](https://github.com/crate-ci/cargo-release). Install it first:

```bash
cargo install cargo-release
```

To create a release:

```bash
# Dry run (no changes made)
cargo release patch    # 0.1.0 -> 0.1.1
cargo release minor    # 0.1.0 -> 0.2.0
cargo release major    # 0.1.0 -> 1.0.0

# Execute for real
cargo release patch --execute
```

This will:
1. Update `version` in all `Cargo.toml` files
2. Create a commit with the version bump
3. Create a git tag (e.g., `v0.1.1`)
4. Push the commit and tag to the remote

The pushed tag triggers `.github/workflows/release.yml`, which:
1. Creates a GitHub Release
2. Builds binaries for Linux (x86_64, aarch64), macOS (x86_64, aarch64), and Windows (x86_64) with [upload-rust-binary-action](https://github.com/taiki-e/upload-rust-binary-action) and attaches them with SHA-256 checksums
3. Updates the Homebrew formula in `hirosassa/homebrew-tap` (skipped for prereleases)

Pull requests that touch the release workflow or the crates run the same builds in dry-run mode (nothing is uploaded).

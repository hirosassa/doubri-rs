#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "test code"
)]

//! Failure-path contracts of the public API: every I/O or format failure must surface as the
//! matching `DoubriError` variant instead of being swallowed or turned into a panic.

use std::fmt::Write as _;
use std::io::{self, BufReader, Write};
use std::path::{Path, PathBuf};

use doubri_core::apply::{apply_each, apply_whole};
use doubri_core::dedup::dedup_group;
use doubri_core::error::DoubriError;
use doubri_core::format::{FLAG_UNIQUE, HashFileHeader, write_hash_header};
use doubri_core::merge::{MergeSource, merge_groups};
use doubri_core::minhash::{MinHashConfig, process_jsonl};
use doubri_core::similarity::compute_pairwise_similarity;
use tempfile::TempDir;

/// A sink that always fails, standing in for a full disk or closed pipe.
struct FailingWriter;

impl Write for FailingWriter {
    fn write(&mut self, _buf: &[u8]) -> io::Result<usize> {
        Err(io::Error::new(io::ErrorKind::BrokenPipe, "sink closed"))
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

const fn small_config() -> MinHashConfig {
    MinHashConfig {
        ngram_size: 3,
        num_buckets: 2,
        band_size: 2,
    }
}

fn write_hash_file(dir: &Path, name: &str, config: &MinHashConfig, texts: &[&str]) -> PathBuf {
    let mut jsonl = String::new();
    for t in texts {
        writeln!(jsonl, "{{\"text\":\"{t}\"}}").unwrap();
    }
    let path = dir.join(name);
    let mut out = Vec::new();
    process_jsonl(jsonl.as_bytes(), &mut out, config, "text").unwrap();
    std::fs::write(&path, out).unwrap();
    path
}

#[test]
fn apply_whole_propagates_writer_failure_as_io_error() {
    let err = apply_whole(
        BufReader::new(&b"{\"text\":\"a\"}\n"[..]),
        &mut FailingWriter,
        &[FLAG_UNIQUE],
    )
    .unwrap_err();
    assert!(matches!(err, DoubriError::Io(ref e) if e.kind() == io::ErrorKind::BrokenPipe));
}

#[test]
fn apply_each_propagates_reader_failure_as_io_error() {
    // Invalid UTF-8 makes `BufRead::lines` fail.
    let err = apply_each(
        BufReader::new(&[0xff, 0xfe, b'\n'][..]),
        &mut Vec::new(),
        &[FLAG_UNIQUE],
        0,
    )
    .unwrap_err();
    assert!(matches!(err, DoubriError::Io(ref e) if e.kind() == io::ErrorKind::InvalidData));
}

#[test]
fn apply_each_does_not_count_documents_whose_write_failed() {
    // The failure must abort the run rather than report a partial count as success.
    let result = apply_each(
        BufReader::new(&b"{\"text\":\"a\"}\n{\"text\":\"b\"}\n"[..]),
        &mut FailingWriter,
        &[FLAG_UNIQUE, FLAG_UNIQUE],
        0,
    );
    assert!(result.is_err());
}

#[test]
fn dedup_group_rejects_empty_file_list() {
    let dir = TempDir::new().unwrap();
    let base = dir.path().join("out");
    let err = dedup_group(&[], base.to_str().unwrap(), false).unwrap_err();
    assert!(matches!(err, DoubriError::Config { .. }));
}

#[test]
fn dedup_group_reports_missing_hash_file_as_io_error() {
    let dir = TempDir::new().unwrap();
    let base = dir.path().join("out");
    let err = dedup_group(
        &[dir.path().join("absent.hash")],
        base.to_str().unwrap(),
        false,
    )
    .unwrap_err();
    assert!(matches!(err, DoubriError::Io(ref e) if e.kind() == io::ErrorKind::NotFound));
}

#[test]
fn dedup_group_rejects_truncated_signature_data() {
    let dir = TempDir::new().unwrap();
    let config = small_config();
    let path = write_hash_file(dir.path(), "a.hash", &config, &["hello world foo"]);
    let mut bytes = std::fs::read(&path).unwrap();
    bytes.truncate(bytes.len() - 8);
    std::fs::write(&path, bytes).unwrap();

    let base = dir.path().join("out");
    let err = dedup_group(&[path], base.to_str().unwrap(), false).unwrap_err();
    assert!(matches!(err, DoubriError::Io(ref e) if e.kind() == io::ErrorKind::UnexpectedEof));
}

#[test]
fn dedup_group_rejects_files_with_different_configs() {
    let dir = TempDir::new().unwrap();
    let a = write_hash_file(dir.path(), "a.hash", &small_config(), &["hello world foo"]);
    let other = MinHashConfig {
        band_size: 3,
        ..small_config()
    };
    let b = write_hash_file(dir.path(), "b.hash", &other, &["hello world foo"]);

    let base = dir.path().join("out");
    let err = dedup_group(&[a, b], base.to_str().unwrap(), false).unwrap_err();
    assert!(matches!(err, DoubriError::Config { .. }));
}

#[test]
fn dedup_group_rejects_header_with_invalid_magic() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("bad.hash");
    std::fs::write(&path, b"NOTDOUBRI-----------------------").unwrap();

    let base = dir.path().join("out");
    let err = dedup_group(&[path], base.to_str().unwrap(), false).unwrap_err();
    assert!(matches!(err, DoubriError::InvalidFormat { .. }));
}

#[test]
fn dedup_group_rejects_header_declaring_more_documents_than_present() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("lying.hash");
    let mut out = Vec::new();
    write_hash_header(
        &mut out,
        &HashFileHeader {
            num_documents: 1_000_000,
            ngram_size: 3,
            num_buckets: 2,
            band_size: 2,
        },
    )
    .unwrap();
    std::fs::write(&path, out).unwrap();

    let base = dir.path().join("out");
    let err = dedup_group(&[path], base.to_str().unwrap(), false).unwrap_err();
    assert!(matches!(err, DoubriError::Io(ref e) if e.kind() == io::ErrorKind::UnexpectedEof));
}

#[test]
fn merge_groups_reports_missing_dup_file_as_io_error() {
    let dir = TempDir::new().unwrap();
    let sources = [MergeSource {
        basename: dir.path().join("absent").to_str().unwrap().to_string(),
        doc_count: 1,
    }];
    let err = merge_groups(&sources, 0, 255, false).unwrap_err();
    assert!(matches!(err, DoubriError::Io(ref e) if e.kind() == io::ErrorKind::NotFound));
}

#[test]
fn merge_groups_rejects_index_file_with_invalid_magic() {
    let dir = TempDir::new().unwrap();
    let base = dir.path().join("g").to_str().unwrap().to_string();
    std::fs::write(format!("{base}.dup"), " ").unwrap();
    std::fs::write(format!("{base}.idx.00000"), b"WRONGMAG").unwrap();
    let sources = [MergeSource {
        basename: base,
        doc_count: 1,
    }];
    let err = merge_groups(&sources, 0, 0, false).unwrap_err();
    assert!(matches!(err, DoubriError::InvalidFormat { .. }));
}

#[test]
fn pairwise_similarity_rejects_missing_text_field() {
    let err = compute_pairwise_similarity(
        BufReader::new(&b"{\"id\":\"x\"}\n"[..]),
        &mut Vec::new(),
        3,
        0.5,
        "id",
        "text",
    )
    .unwrap_err();
    assert!(matches!(err, DoubriError::MissingField { ref field } if field == "text"));
}

#[test]
fn pairwise_similarity_rejects_malformed_json() {
    let err = compute_pairwise_similarity(
        BufReader::new(&b"not json\n"[..]),
        &mut Vec::new(),
        3,
        0.5,
        "id",
        "text",
    )
    .unwrap_err();
    assert!(matches!(err, DoubriError::Json(_)));
}

#[test]
fn pairwise_similarity_propagates_writer_failure() {
    let input =
        b"{\"id\":\"a\",\"text\":\"hello world\"}\n{\"id\":\"b\",\"text\":\"hello world\"}\n";
    let err = compute_pairwise_similarity(
        BufReader::new(&input[..]),
        &mut FailingWriter,
        3,
        0.5,
        "id",
        "text",
    )
    .unwrap_err();
    assert!(matches!(err, DoubriError::Io(_)));
}

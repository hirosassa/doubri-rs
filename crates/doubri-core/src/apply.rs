use std::io::{BufRead, Write};

use crate::error::DoubriError;
use crate::format::FLAG_UNIQUE;

/// Outputs only non-duplicate documents based on duplicate flags (all sources at once).
///
/// Each JSONL line is checked against the flags; only lines with `FLAG_UNIQUE` are output.
///
/// # Errors
///
/// Returns an error on I/O failure or when the input has more documents than `dup_flags`.
pub fn apply_whole<R: BufRead, W: Write>(
    reader: R,
    writer: &mut W,
    dup_flags: &[u8],
) -> Result<u64, DoubriError> {
    apply_from(reader, writer, dup_flags, 0, "more documents than flags")
}

/// Outputs only non-duplicate documents for a specific source based on duplicate flags.
///
/// - `reader`: JSONL stream for the target source.
/// - `dup_flags`: full flag array.
/// - `offset`: starting position of this source within the flag array.
///
/// # Errors
///
/// Returns an error on I/O failure or when a document index (offset included) is outside
/// `dup_flags`.
pub fn apply_each<R: BufRead, W: Write>(
    reader: R,
    writer: &mut W,
    dup_flags: &[u8],
    offset: usize,
) -> Result<u64, DoubriError> {
    apply_from(
        reader,
        writer,
        dup_flags,
        offset,
        "document index exceeds flag count",
    )
}

fn apply_from<R: BufRead, W: Write>(
    reader: R,
    writer: &mut W,
    dup_flags: &[u8],
    offset: usize,
    exceed_msg: &str,
) -> Result<u64, DoubriError> {
    let mut output_count = 0u64;
    let mut flag_idx = offset;

    for line in reader.lines() {
        let line = line?;
        if line.is_empty() {
            continue;
        }

        let flag = dup_flags
            .get(flag_idx)
            .ok_or_else(|| DoubriError::InvalidFormat {
                msg: format!(
                    "{exceed_msg}: index {flag_idx}, flag count {}",
                    dup_flags.len()
                ),
            })?;

        if *flag == FLAG_UNIQUE {
            writeln!(writer, "{line}")?;
            output_count = output_count.checked_add(1).ok_or(DoubriError::Overflow {
                context: "apply output count",
            })?;
        }

        flag_idx = flag_idx.checked_add(1).ok_or(DoubriError::Overflow {
            context: "apply flag index",
        })?;
    }

    Ok(output_count)
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
    use crate::format::{FLAG_DUPLICATE, FLAG_UNIQUE};

    #[test]
    fn test_apply_whole_all_unique() {
        let jsonl = r#"{"text":"doc1"}
{"text":"doc2"}
{"text":"doc3"}
"#;
        let flags = vec![FLAG_UNIQUE, FLAG_UNIQUE, FLAG_UNIQUE];
        let mut output = Vec::new();
        let count = apply_whole(
            std::io::BufReader::new(jsonl.as_bytes()),
            &mut output,
            &flags,
        )
        .unwrap();

        assert_eq!(count, 3);
        let output_str = String::from_utf8(output).unwrap();
        assert!(output_str.contains("doc1"));
        assert!(output_str.contains("doc2"));
        assert!(output_str.contains("doc3"));
    }

    #[test]
    fn test_apply_whole_with_duplicates() {
        let jsonl = r#"{"text":"doc1"}
{"text":"doc2"}
{"text":"doc3"}
"#;
        let flags = vec![FLAG_UNIQUE, FLAG_DUPLICATE, FLAG_UNIQUE];
        let mut output = Vec::new();
        let count = apply_whole(
            std::io::BufReader::new(jsonl.as_bytes()),
            &mut output,
            &flags,
        )
        .unwrap();

        assert_eq!(count, 2);
        let output_str = String::from_utf8(output).unwrap();
        assert!(output_str.contains("doc1"));
        assert!(!output_str.contains("doc2"));
        assert!(output_str.contains("doc3"));
    }

    #[test]
    fn test_apply_whole_all_duplicates() {
        let jsonl = r#"{"text":"doc1"}
{"text":"doc2"}
"#;
        let flags = vec![FLAG_DUPLICATE, FLAG_DUPLICATE];
        let mut output = Vec::new();
        let count = apply_whole(
            std::io::BufReader::new(jsonl.as_bytes()),
            &mut output,
            &flags,
        )
        .unwrap();

        assert_eq!(count, 0);
        assert_eq!(output, [] as [u8; 0]);
    }

    #[test]
    fn test_apply_whole_more_docs_than_flags() {
        let jsonl = r#"{"text":"doc1"}
{"text":"doc2"}
{"text":"doc3"}
"#;
        let flags = vec![FLAG_UNIQUE, FLAG_UNIQUE];
        let mut output = Vec::new();
        let result = apply_whole(
            std::io::BufReader::new(jsonl.as_bytes()),
            &mut output,
            &flags,
        );
        assert!(result.is_err());
    }

    #[test]
    fn test_apply_each_basic() {
        let jsonl = r#"{"text":"doc_a"}
{"text":"doc_b"}
"#;
        // Full group flags: [UNIQUE, DUP, UNIQUE, DUP]
        // This source starts at offset=2
        let flags = vec![FLAG_UNIQUE, FLAG_DUPLICATE, FLAG_UNIQUE, FLAG_DUPLICATE];
        let mut output = Vec::new();
        let count = apply_each(
            std::io::BufReader::new(jsonl.as_bytes()),
            &mut output,
            &flags,
            2,
        )
        .unwrap();

        assert_eq!(count, 1);
        let output_str = String::from_utf8(output).unwrap();
        assert!(output_str.contains("doc_a"));
        assert!(!output_str.contains("doc_b"));
    }

    #[test]
    fn test_apply_each_offset_zero() {
        let jsonl = r#"{"text":"doc1"}
{"text":"doc2"}
"#;
        let flags = vec![FLAG_DUPLICATE, FLAG_UNIQUE];
        let mut output = Vec::new();
        let count = apply_each(
            std::io::BufReader::new(jsonl.as_bytes()),
            &mut output,
            &flags,
            0,
        )
        .unwrap();

        assert_eq!(count, 1);
        let output_str = String::from_utf8(output).unwrap();
        assert!(!output_str.contains("doc1"));
        assert!(output_str.contains("doc2"));
    }

    #[test]
    fn test_apply_each_flag_index_out_of_bounds() {
        // offset + doc_idx runs past the flag array on the second document.
        let jsonl = "{\"text\":\"doc1\"}\n{\"text\":\"doc2\"}\n";
        let flags = vec![FLAG_UNIQUE, FLAG_UNIQUE];
        let mut output = Vec::new();
        let err = apply_each(
            std::io::BufReader::new(jsonl.as_bytes()),
            &mut output,
            &flags,
            1,
        )
        .unwrap_err();
        assert!(
            matches!(err, DoubriError::InvalidFormat { .. }),
            "expected InvalidFormat when doc index exceeds flags, got {err:?}"
        );
    }

    #[test]
    fn test_apply_whole_empty_lines_skipped() {
        let jsonl = "{\"text\":\"doc1\"}\n\n{\"text\":\"doc2\"}\n";
        let flags = vec![FLAG_UNIQUE, FLAG_UNIQUE];
        let mut output = Vec::new();
        let count = apply_whole(
            std::io::BufReader::new(jsonl.as_bytes()),
            &mut output,
            &flags,
        )
        .unwrap();
        assert_eq!(count, 2);
    }

    #[test]
    fn test_apply_each_empty_lines_do_not_consume_flags() {
        // Empty lines are not documents, so they must not advance the flag index:
        // doc_a uses flags[1] (duplicate), doc_b uses flags[2] (unique).
        let jsonl = "{\"text\":\"doc_a\"}\n\n{\"text\":\"doc_b\"}\n";
        let flags = vec![FLAG_UNIQUE, FLAG_DUPLICATE, FLAG_UNIQUE];
        let mut output = Vec::new();
        let count = apply_each(
            std::io::BufReader::new(jsonl.as_bytes()),
            &mut output,
            &flags,
            1,
        )
        .unwrap();

        assert_eq!(count, 1);
        assert_eq!(String::from_utf8(output).unwrap(), "{\"text\":\"doc_b\"}\n");
    }

    #[test]
    fn test_apply_each_offset_at_usize_max_is_error() {
        let mut output = Vec::new();
        let err = apply_each(
            std::io::BufReader::new(&b"{\"text\":\"doc\"}\n"[..]),
            &mut output,
            &[FLAG_UNIQUE],
            usize::MAX,
        )
        .unwrap_err();
        assert!(matches!(err, DoubriError::InvalidFormat { .. }));
    }
}

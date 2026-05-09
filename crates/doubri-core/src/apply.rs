use std::io::{BufRead, Write};

use crate::error::DoubriError;
use crate::format::FLAG_UNIQUE;

/// Outputs only non-duplicate documents based on duplicate flags (all sources at once).
///
/// Each JSONL line is checked against the flags; only lines with `FLAG_UNIQUE` are output.
pub fn apply_whole<R: BufRead, W: Write>(
    reader: R,
    writer: &mut W,
    dup_flags: &[u8],
) -> Result<u64, DoubriError> {
    let mut output_count = 0u64;
    let mut line_idx = 0usize;

    for line in reader.lines() {
        let line = line?;
        if line.is_empty() {
            continue;
        }

        if line_idx >= dup_flags.len() {
            return Err(DoubriError::InvalidFormat {
                msg: format!(
                    "more documents ({}) than flags ({})",
                    line_idx + 1,
                    dup_flags.len()
                ),
            });
        }

        if dup_flags[line_idx] == FLAG_UNIQUE {
            writeln!(writer, "{}", line)?;
            output_count += 1;
        }

        line_idx += 1;
    }

    Ok(output_count)
}

/// Outputs only non-duplicate documents for a specific source based on duplicate flags.
///
/// - `reader`: JSONL stream for the target source.
/// - `dup_flags`: full flag array.
/// - `offset`: starting position of this source within the flag array.
pub fn apply_each<R: BufRead, W: Write>(
    reader: R,
    writer: &mut W,
    dup_flags: &[u8],
    offset: usize,
) -> Result<u64, DoubriError> {
    let mut output_count = 0u64;
    let mut doc_idx = 0usize;

    for line in reader.lines() {
        let line = line?;
        if line.is_empty() {
            continue;
        }

        let flag_idx = offset + doc_idx;
        if flag_idx >= dup_flags.len() {
            return Err(DoubriError::InvalidFormat {
                msg: format!(
                    "document index {} exceeds flag count {}",
                    flag_idx,
                    dup_flags.len()
                ),
            });
        }

        if dup_flags[flag_idx] == FLAG_UNIQUE {
            writeln!(writer, "{}", line)?;
            output_count += 1;
        }

        doc_idx += 1;
    }

    Ok(output_count)
}

#[cfg(test)]
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
        assert!(output.is_empty());
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
}

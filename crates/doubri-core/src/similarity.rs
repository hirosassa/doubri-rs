use std::collections::HashSet;
use std::io::{BufRead, Write};

use crate::error::DoubriError;
use crate::ngram::char_ngrams;

/// Computes character n-gram based Jaccard similarity between two texts.
pub fn jaccard_similarity(text_a: &str, text_b: &str, ngram_size: usize) -> f64 {
    let set_a: HashSet<&str> = char_ngrams(text_a, ngram_size).collect();
    let set_b: HashSet<&str> = char_ngrams(text_b, ngram_size).collect();

    if set_a.is_empty() && set_b.is_empty() {
        return 1.0;
    }
    if set_a.is_empty() || set_b.is_empty() {
        return 0.0;
    }

    let intersection = set_a.intersection(&set_b).count();
    let union = set_a.len() + set_b.len() - intersection;

    intersection as f64 / union as f64
}

/// Computes pairwise Jaccard similarity for all document pairs from a JSONL stream,
/// outputting pairs above the threshold.
pub fn compute_pairwise_similarity<R: BufRead, W: Write>(
    reader: R,
    writer: &mut W,
    ngram_size: usize,
    threshold: f64,
    id_field: &str,
    text_field: &str,
) -> Result<u64, DoubriError> {
    let mut documents: Vec<(String, String)> = Vec::new(); // (id, text)

    for line in reader.lines() {
        let line = line?;
        if line.is_empty() {
            continue;
        }
        let json: serde_json::Value = serde_json::from_str(&line)?;
        let id = json
            .get(id_field)
            .map(|v| match v {
                serde_json::Value::String(s) => s.clone(),
                other => other.to_string(),
            })
            .unwrap_or_else(|| documents.len().to_string());
        let text = json
            .get(text_field)
            .and_then(|v| v.as_str())
            .ok_or_else(|| DoubriError::MissingField {
                field: text_field.to_string(),
            })?
            .to_string();
        documents.push((id, text));
    }

    let mut pair_count = 0u64;

    for i in 0..documents.len() {
        for j in (i + 1)..documents.len() {
            let sim = jaccard_similarity(&documents[i].1, &documents[j].1, ngram_size);
            if sim >= threshold {
                writeln!(writer, "{:.6}\t{}\t{}", sim, documents[i].0, documents[j].0)?;
                pair_count += 1;
            }
        }
    }

    Ok(pair_count)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_identical_texts() {
        let sim = jaccard_similarity("hello world", "hello world", 3);
        assert!((sim - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_completely_different_texts() {
        let sim = jaccard_similarity("aaaaaaa", "bbbbbbb", 3);
        assert!((sim - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_partially_similar_texts() {
        let sim = jaccard_similarity("hello world", "hello earth", 3);
        assert!(sim > 0.0);
        assert!(sim < 1.0);
    }

    #[test]
    fn test_empty_texts() {
        let sim = jaccard_similarity("", "", 3);
        assert!((sim - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_one_empty_text() {
        let sim = jaccard_similarity("hello world", "", 3);
        assert!((sim - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_known_jaccard_value() {
        // "abcde" 2-grams: {"ab", "bc", "cd", "de"} (4)
        // "bcdef" 2-grams: {"bc", "cd", "de", "ef"} (4)
        // Intersection: {"bc", "cd", "de"} (3)
        // Union: {"ab", "bc", "cd", "de", "ef"} (5)
        // Jaccard = 3/5 = 0.6
        let sim = jaccard_similarity("abcde", "bcdef", 2);
        assert!((sim - 0.6).abs() < f64::EPSILON);
    }

    #[test]
    fn test_unicode_jaccard() {
        let sim = jaccard_similarity("こんにちは世界", "こんにちは日本", 3);
        assert!(sim > 0.0);
        assert!(sim < 1.0);
    }

    #[test]
    fn test_pairwise_similarity_basic() {
        let jsonl = r#"{"id":"doc1","text":"hello world foo bar"}
{"id":"doc2","text":"hello world foo baz"}
{"id":"doc3","text":"completely different text"}
"#;

        let mut output = Vec::new();
        let count = compute_pairwise_similarity(
            std::io::BufReader::new(jsonl.as_bytes()),
            &mut output,
            3,
            0.3,
            "id",
            "text",
        )
        .unwrap();

        let output_str = String::from_utf8(output).unwrap();
        // doc1 and doc2 should have high similarity
        assert!(count >= 1);
        assert!(output_str.contains("doc1"));
        assert!(output_str.contains("doc2"));
    }

    #[test]
    fn test_pairwise_similarity_high_threshold() {
        let jsonl = r#"{"id":"doc1","text":"hello world"}
{"id":"doc2","text":"goodbye world"}
"#;

        let mut output = Vec::new();
        let count = compute_pairwise_similarity(
            std::io::BufReader::new(jsonl.as_bytes()),
            &mut output,
            3,
            0.99,
            "id",
            "text",
        )
        .unwrap();

        // Threshold too high, no matches
        assert_eq!(count, 0);
    }
}

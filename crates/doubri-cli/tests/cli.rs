//! End-to-end tests that run the real `doubri` binary.

use std::io::Write;
use std::path::Path;
use std::process::{Command, Output, Stdio};

use tempfile::TempDir;

const TEXT_A: &str = "The quick brown fox jumps over the lazy dog near the quiet river bank";
const TEXT_B: &str =
    "Completely unrelated sentence about distributed systems and consensus protocols";
const TEXT_C: &str =
    "Another distinct document describing how tea is harvested in mountain regions";

fn doubri(dir: &Path, args: &[&str], stdin: &str) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_doubri"))
        .args(args)
        .current_dir(dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to spawn doubri");
    child
        .stdin
        .take()
        .expect("stdin is piped")
        .write_all(stdin.as_bytes())
        .expect("failed to write stdin");
    child.wait_with_output().expect("failed to wait for doubri")
}

fn stdout(out: &Output) -> String {
    String::from_utf8(out.stdout.clone()).expect("stdout is UTF-8")
}

fn stderr(out: &Output) -> String {
    String::from_utf8(out.stderr.clone()).expect("stderr is UTF-8")
}

fn doc(id: &str, text: &str) -> String {
    format!("{{\"id\":\"{id}\",\"text\":\"{text}\"}}")
}

fn jsonl(docs: &[String]) -> String {
    docs.iter().map(|d| format!("{d}\n")).collect()
}

fn lines(out: &Output) -> Vec<String> {
    stdout(out).lines().map(str::to_owned).collect()
}

fn run_ok(dir: &Path, args: &[&str], stdin: &str) -> Output {
    let out = doubri(dir, args, stdin);
    assert!(
        out.status.success(),
        "doubri {args:?} failed: {}",
        stderr(&out)
    );
    out
}

#[test]
fn pipeline_removes_exact_duplicate_and_keeps_first() {
    let dir = TempDir::new().unwrap();
    let docs = [doc("d0", TEXT_A), doc("d1", TEXT_B), doc("d2", TEXT_A)];
    let input = jsonl(&docs);

    run_ok(dir.path(), &["minhash", "g.hash"], &input);
    run_ok(dir.path(), &["dedup", "g"], "g.hash\n");
    let out = run_ok(dir.path(), &["apply-whole", "-f", "g.dup"], &input);

    assert_eq!(lines(&out), vec![docs[0].clone(), docs[1].clone()]);
    assert!(stderr(&out).contains("Output 2 documents"));
}

#[test]
fn reverse_keeps_last_of_duplicates() {
    let dir = TempDir::new().unwrap();
    let docs = [doc("d0", TEXT_A), doc("d1", TEXT_B), doc("d2", TEXT_A)];
    let input = jsonl(&docs);

    run_ok(dir.path(), &["minhash", "g.hash"], &input);
    run_ok(dir.path(), &["dedup", "--reverse", "g"], "g.hash\n");
    let out = run_ok(dir.path(), &["apply-whole", "-f", "g.dup"], &input);

    assert_eq!(lines(&out), vec![docs[1].clone(), docs[2].clone()]);
}

#[test]
fn dedup_reports_duplicate_summary() {
    let dir = TempDir::new().unwrap();
    let input = jsonl(&[doc("d0", TEXT_A), doc("d1", TEXT_B), doc("d2", TEXT_A)]);

    run_ok(dir.path(), &["minhash", "g.hash"], &input);
    let out = run_ok(dir.path(), &["dedup", "g"], "g.hash\n");

    assert!(
        stderr(&out).contains("Total: 3 documents, 1 duplicates (33.3%)"),
        "unexpected summary: {}",
        stderr(&out)
    );
}

#[test]
fn dedup_without_duplicates_keeps_every_document() {
    let dir = TempDir::new().unwrap();
    let docs = [doc("d0", TEXT_A), doc("d1", TEXT_B), doc("d2", TEXT_C)];
    let input = jsonl(&docs);

    run_ok(dir.path(), &["minhash", "g.hash"], &input);
    run_ok(dir.path(), &["dedup", "g"], "g.hash\n");
    let out = run_ok(dir.path(), &["apply-whole", "-f", "g.dup"], &input);

    assert_eq!(lines(&out), docs.to_vec());
}

#[test]
fn single_document_is_kept() {
    let dir = TempDir::new().unwrap();
    let docs = [doc("d0", TEXT_A)];
    let input = jsonl(&docs);

    run_ok(dir.path(), &["minhash", "g.hash"], &input);
    run_ok(dir.path(), &["dedup", "g"], "g.hash\n");
    let out = run_ok(dir.path(), &["apply-whole", "-f", "g.dup"], &input);

    assert_eq!(lines(&out), docs.to_vec());
}

#[test]
fn merge_removes_duplicates_across_groups_and_keeps_older_group() {
    let dir = TempDir::new().unwrap();
    let group1 = [doc("a0", TEXT_A), doc("a1", TEXT_B)];
    let group2 = [doc("b0", TEXT_A), doc("b1", TEXT_C)];

    for (name, docs) in [("g1", &group1), ("g2", &group2)] {
        let hash = format!("{name}.hash");
        run_ok(dir.path(), &["minhash", &hash], &jsonl(docs));
        run_ok(dir.path(), &["dedup", name], &format!("{hash}\n"));
    }
    let merged = run_ok(dir.path(), &["merge", "g1", "g2"], "");
    assert!(stderr(&merged).contains("Cross-group duplicates:"));

    let kept1 = run_ok(
        dir.path(),
        &["apply-whole", "-f", "g1.dup.merge"],
        &jsonl(&group1),
    );
    let kept2 = run_ok(
        dir.path(),
        &["apply-whole", "-f", "g2.dup.merge"],
        &jsonl(&group2),
    );

    assert_eq!(lines(&kept1), group1.to_vec());
    assert_eq!(lines(&kept2), vec![group2[1].clone()]);
}

#[test]
fn merge_reverse_keeps_newer_group() {
    let dir = TempDir::new().unwrap();
    let group1 = [doc("a0", TEXT_A), doc("a1", TEXT_B)];
    let group2 = [doc("b0", TEXT_A), doc("b1", TEXT_C)];

    for (name, docs) in [("g1", &group1), ("g2", &group2)] {
        let hash = format!("{name}.hash");
        run_ok(dir.path(), &["minhash", &hash], &jsonl(docs));
        run_ok(dir.path(), &["dedup", name], &format!("{hash}\n"));
    }
    run_ok(dir.path(), &["merge", "--reverse", "g1", "g2"], "");

    let kept1 = run_ok(
        dir.path(),
        &["apply-whole", "-f", "g1.dup.merge"],
        &jsonl(&group1),
    );
    let kept2 = run_ok(
        dir.path(),
        &["apply-whole", "-f", "g2.dup.merge"],
        &jsonl(&group2),
    );

    assert_eq!(lines(&kept1), vec![group1[1].clone()]);
    assert_eq!(lines(&kept2), group2.to_vec());
}

#[test]
fn apply_each_filters_each_source_by_its_offset() {
    let dir = TempDir::new().unwrap();
    let source1 = [doc("a0", TEXT_A), doc("a1", TEXT_B)];
    let source2 = [doc("b0", TEXT_A), doc("b1", TEXT_C)];

    run_ok(dir.path(), &["minhash", "s1.hash"], &jsonl(&source1));
    run_ok(dir.path(), &["minhash", "s2.hash"], &jsonl(&source2));
    run_ok(dir.path(), &["dedup", "g"], "s1.hash\ns2.hash\n");

    let kept1 = run_ok(
        dir.path(),
        &["apply-each", "-f", "g.dup", "-s", "g.src", "s1.hash"],
        &jsonl(&source1),
    );
    let kept2 = run_ok(
        dir.path(),
        &["apply-each", "-f", "g.dup", "-s", "g.src", "s2.hash"],
        &jsonl(&source2),
    );

    assert_eq!(lines(&kept1), source1.to_vec());
    assert_eq!(lines(&kept2), vec![source2[1].clone()]);
}

#[test]
fn apply_each_rejects_target_missing_from_source_list() {
    let dir = TempDir::new().unwrap();
    let input = jsonl(&[doc("d0", TEXT_A)]);

    run_ok(dir.path(), &["minhash", "s1.hash"], &input);
    run_ok(dir.path(), &["dedup", "g"], "s1.hash\n");
    let out = doubri(
        dir.path(),
        &["apply-each", "-f", "g.dup", "-s", "g.src", "other.hash"],
        &input,
    );

    assert!(!out.status.success());
    assert!(
        stderr(&out).contains("target 'other.hash' not found"),
        "unexpected error: {}",
        stderr(&out)
    );
}

#[test]
fn similarity_reports_only_pairs_above_threshold() {
    let dir = TempDir::new().unwrap();
    let input = jsonl(&[doc("d0", TEXT_A), doc("d1", TEXT_B), doc("d2", TEXT_A)]);

    let out = run_ok(dir.path(), &["similarity"], &input);

    assert_eq!(lines(&out), vec!["1.000000\td0\td2".to_owned()]);
    assert!(stderr(&out).contains("Found 1 pairs above threshold 0.6"));
}

#[test]
fn similarity_uses_custom_id_and_text_fields() {
    let dir = TempDir::new().unwrap();
    let input = format!(
        "{{\"key\":\"x\",\"body\":\"{TEXT_A}\"}}\n{{\"key\":\"y\",\"body\":\"{TEXT_A}\"}}\n"
    );

    let out = run_ok(
        dir.path(),
        &["similarity", "--id-field", "key", "--text-field", "body"],
        &input,
    );

    assert_eq!(lines(&out), vec!["1.000000\tx\ty".to_owned()]);
}

#[test]
fn dedup_rejects_empty_hash_file_list() {
    let dir = TempDir::new().unwrap();

    let out = doubri(dir.path(), &["dedup", "g"], "");

    assert!(!out.status.success());
    assert!(stderr(&out).contains("no hash files provided on stdin"));
}

#[test]
fn minhash_rejects_invalid_json() {
    let dir = TempDir::new().unwrap();

    let out = doubri(dir.path(), &["minhash", "g.hash"], "not json\n");

    assert!(!out.status.success());
    assert!(stderr(&out).contains("minhash failed"));
}

#[test]
fn minhash_rejects_document_without_text_field() {
    let dir = TempDir::new().unwrap();
    let input = jsonl(&[doc("d0", TEXT_A)]);

    let out = doubri(
        dir.path(),
        &["minhash", "--text-field", "body", "g.hash"],
        &input,
    );

    assert!(!out.status.success());
    assert!(
        stderr(&out).contains("body"),
        "error should name the missing field: {}",
        stderr(&out)
    );
}

#[test]
fn minhash_fails_when_output_cannot_be_created() {
    let dir = TempDir::new().unwrap();

    let out = doubri(
        dir.path(),
        &["minhash", "missing-dir/g.hash"],
        &jsonl(&[doc("d0", TEXT_A)]),
    );

    assert!(!out.status.success());
    assert!(stderr(&out).contains("failed to create missing-dir/g.hash"));
}

#[test]
fn apply_whole_fails_when_flag_file_is_missing() {
    let dir = TempDir::new().unwrap();

    let out = doubri(dir.path(), &["apply-whole", "-f", "nope.dup"], "");

    assert!(!out.status.success());
}

#[test]
fn merge_without_sources_fails() {
    let dir = TempDir::new().unwrap();

    let out = doubri(dir.path(), &["merge"], "");

    assert!(!out.status.success());
    assert!(stderr(&out).contains("no source basenames provided"));
}

#[test]
fn dedup_of_empty_input_does_not_report_nan_percentage() {
    let dir = TempDir::new().unwrap();

    run_ok(dir.path(), &["minhash", "g.hash"], "");
    let out = doubri(dir.path(), &["dedup", "g"], "g.hash\n");

    assert!(
        !stderr(&out).contains("NaN"),
        "summary should not contain NaN: {}",
        stderr(&out)
    );
}

#[test]
fn unknown_subcommand_is_rejected() {
    let dir = TempDir::new().unwrap();

    let out = doubri(dir.path(), &["frobnicate"], "");

    assert!(!out.status.success());
}

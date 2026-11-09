use serde_json::Value;
use std::fs;
use std::process::{Command, Output};
use tempfile::tempdir;

fn run(directory: &std::path::Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_kolark"))
        .current_dir(directory)
        .args(args)
        .output()
        .expect("run kolark CLI")
}

#[test]
fn create_is_atomic_and_refuses_overwrite_without_force() {
    let directory = tempdir().unwrap();
    fs::write(
        directory.path().join("input.txt"),
        b"KolArk CLI integration",
    )
    .unwrap();

    let created = run(
        directory.path(),
        &["--json", "create", "archive.kola", "input.txt"],
    );
    assert!(
        created.status.success(),
        "{}",
        String::from_utf8_lossy(&created.stderr)
    );
    let response: Value = serde_json::from_slice(&created.stdout).unwrap();
    assert_eq!(response["files"], 1);
    let original = fs::read(directory.path().join("archive.kola")).unwrap();

    let refused = run(directory.path(), &["create", "archive.kola", "input.txt"]);
    assert_eq!(refused.status.code(), Some(8));
    assert_eq!(
        fs::read(directory.path().join("archive.kola")).unwrap(),
        original
    );

    let failed_replacement = run(
        directory.path(),
        &["create", "archive.kola", "missing.txt", "--force"],
    );
    assert_eq!(failed_replacement.status.code(), Some(5));
    assert_eq!(
        fs::read(directory.path().join("archive.kola")).unwrap(),
        original
    );
}

#[test]
fn list_verify_and_extract_work_with_json_output() {
    let directory = tempdir().unwrap();
    fs::write(directory.path().join("input.txt"), b"archive contents").unwrap();
    let created = run(directory.path(), &["create", "archive.kola", "input.txt"]);
    assert!(
        created.status.success(),
        "{}",
        String::from_utf8_lossy(&created.stderr)
    );

    let listed = run(directory.path(), &["--json", "list", "archive.kola"]);
    assert!(
        listed.status.success(),
        "{}",
        String::from_utf8_lossy(&listed.stderr)
    );
    let entries: Value = serde_json::from_slice(&listed.stdout).unwrap();
    assert_eq!(entries[0]["path"], "input.txt");

    let verified = run(directory.path(), &["--json", "verify", "archive.kola"]);
    assert!(
        verified.status.success(),
        "{}",
        String::from_utf8_lossy(&verified.stderr)
    );
    let report: Value = serde_json::from_slice(&verified.stdout).unwrap();
    assert_eq!(report["valid"], true);

    let extracted = run(
        directory.path(),
        &["--json", "extract", "archive.kola", "--directory", "output"],
    );
    assert!(
        extracted.status.success(),
        "{}",
        String::from_utf8_lossy(&extracted.stderr)
    );
    assert_eq!(
        fs::read(directory.path().join("output/input.txt")).unwrap(),
        b"archive contents"
    );
}
